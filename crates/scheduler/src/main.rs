//! Scheduler binary.

mod network;
mod tasks;

use std::{error::Error, fs, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use clap::Parser;
use hypha_network::{
    cert::{
        identity_from_private_key, load_certs_from_pem, load_crls_from_pem,
        load_private_key_from_pem,
    },
    dial::DialInterface,
    gossipsub::GossipsubInterface,
    listen::ListenInterface,
    request_response::{RequestResponseInterface, RequestResponseInterfaceExt},
    swarm::SwarmDriver,
    utils::multiaddr_from_socketaddr,
};
use libp2p::PeerId;
use tokio::{sync::Mutex, task::JoinSet};
use tracing_subscriber::EnvFilter;

use crate::{
    network::Network,
    tasks::{TaskId, Tasks},
};

#[derive(Debug, Parser)]
#[command(
    name = "hypha-scheduler",
    version,
    about = "Hypha Scheduler",
    long_about = "Runs the Hypha Scheduler coordinating workers.",
    after_help = "For more information, see the project documentation."
)]
struct Opt {
    #[clap(long)]
    cert_file: PathBuf,
    #[clap(long)]
    key_file: PathBuf,
    #[clap(long)]
    ca_cert_file: PathBuf,
    #[clap(long)]
    crl_file: Option<PathBuf>,
    #[clap(long)]
    gateway_address: SocketAddr,
    #[clap(long, default_value = "[::1]:0")]
    listen_address: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let opt = Opt::parse();

    // Load certificates and private key
    let cert_pem = fs::read(&opt.cert_file)?;
    let key_pem = fs::read(&opt.key_file)?;
    let ca_cert_pem = fs::read(&opt.ca_cert_file)?;

    let cert_chain = load_certs_from_pem(&cert_pem)?;
    let private_key = load_private_key_from_pem(&key_pem)?;
    let ca_certs = load_certs_from_pem(&ca_cert_pem)?;

    // Optionally load CRLs
    let crls = if let Some(crl_file) = opt.crl_file {
        let crl_pem = fs::read(&crl_file)?;
        load_crls_from_pem(&crl_pem)?
    } else {
        vec![]
    };

    let local_peer_id = identity_from_private_key(&private_key)?
        .public()
        .to_peer_id();
    let gateway_address = multiaddr_from_socketaddr(opt.gateway_address)?;

    let (network, network_driver) = Network::create(cert_chain, private_key, ca_certs, crls)?;
    tokio::spawn(network_driver.run());

    network
        .listen(multiaddr_from_socketaddr(opt.listen_address)?)
        .await?;
    tracing::info!("Successfully listening");

    // Dial the gateway address
    let _gateway_peer_id = network.dial(gateway_address).await?;

    // Wait a bit until DHT bootstrapping is done.
    // Once we receive an 'Identify' message, bootstrapping will start.
    // TODO: Provide a way to wait for this event
    tokio::time::sleep(Duration::from_secs(2)).await;

    let scheduler = Arc::new(Mutex::new(Tasks::new()));

    let handler = network
        .on(|req: &hypha_api::Request| matches!(req, hypha_api::Request::Worker(_)))
        .into_stream()
        .await?;

    let handler_future = {
        let network = network.clone();
        let scheduler = scheduler.clone();

        handler.respond_with_concurrent(None, move |(peer_id, req)| {
            let network = network.clone();
            let scheduler = scheduler.clone();
            async move {
                match req {
                    hypha_api::Request::Worker(worker) => match worker {
                        hypha_api::WorkerRequest::Available { task_id, role } => {
                            tracing::debug!(
                                task_id = %task_id,
                                peer_id = %peer_id,
                                "Received available worker request",
                            );
                            let (participants, parameter_server_peer_id) = if let Some(task) =
                                scheduler.lock().await.get_task(&task_id)
                            {
                                match role {
                                    hypha_api::WorkerRole::TaskExecutor => task.add_worker(peer_id),
                                    hypha_api::WorkerRole::ParameterExecutor => {
                                        task.add_parameter_server(peer_id)
                                    }
                                };
                                (
                                    task.workers().cloned().collect::<Vec<_>>(),
                                    task.parameter_server(),
                                )
                            } else {
                                (Vec::new(), None)
                            };

                            // TODO: This is very simple and hard-coded scheduling:
                            // Once 2 workers and a parameter server have connected, we start a task.
                            // The criteria when and with whom to start a task need to be added later.
                            if participants.len() == 2 {
                                if let Some(parameter_server_peer_id) = parameter_server_peer_id {
                                    tokio::spawn(start_task(
                                        network.clone(),
                                        task_id,
                                        parameter_server_peer_id,
                                        participants,
                                    ));
                                }
                            }

                            hypha_api::Response::Worker(hypha_api::WorkerResponse::Available {})
                        }
                        hypha_api::WorkerRequest::TaskStatus { task_id, status } => {
                            tracing::info!(
                                task_id = %task_id,
                                peer_id = %peer_id,
                                status = ?status,
                                "Received status update",
                            );

                            if let Some(task) = scheduler.lock().await.get_task(&task_id) {
                                task.update_status(&peer_id, status);
                            }

                            // TODO: At some point a task is completed. Once that is the case
                            // a scheduler needs to inform workers about that so that they no longer
                            // keep track of that task/no longer expect messages related to that task.

                            hypha_api::Response::Worker(hypha_api::WorkerResponse::TaskStatus {})
                        }
                    },
                    _ => {
                        panic!("Unexpected request received");
                    }
                }
            }
        })
    };

    tracing::info!("Publishing task announcement");

    let task_id = scheduler.lock().await.create_task();

    let mut data = Vec::new();

    ciborium::into_writer(
        &hypha_api::RequestAnnounce {
            task_id,
            scheduler_peer_id: local_peer_id,
        },
        &mut data,
    )
    .expect("hardcoded task announcement should serialize successfully");

    let _ = network.publish("announce", data).await;

    handler_future.await;

    Ok(())
}

async fn start_task(
    network: Network,
    task_id: TaskId,
    parameter_server_peer_id: PeerId,
    worker_peer_ids: Vec<PeerId>,
) {
    let mut set = JoinSet::new();

    for worker_peer_id in worker_peer_ids {
        tracing::info!(task_id = %task_id, peer_id = %worker_peer_id, "Requesting work");
        let network = network.clone();
        set.spawn(async move {
            network
                .request(
                    worker_peer_id,
                    hypha_api::Request::Scheduler(hypha_api::SchedulerRequest::Work {
                        task_id,
                        parameter_server_peer_id,
                    }),
                )
                .await
        });
    }

    let _result = set.join_all().await;
}
