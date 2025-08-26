//! Scheduler binary.

use std::{fs, sync::Arc};

use clap::{Parser, Subcommand};
use figment::providers::{Env, Format, Serialized, Toml};
use futures_util::future::join_all;
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_network::{
    cert::identity_from_private_key,
    dial::DialInterface,
    gossipsub::GossipsubInterface,
    kad::KademliaInterface,
    listen::ListenInterface,
    request_response::{RequestResponseInterface, RequestResponseInterfaceExt},
    swarm::SwarmDriver,
};
use hypha_scheduler::{
    config::Config,
    network::Network,
    tasks::{TaskId, Tasks},
};
use libp2p::{Multiaddr, PeerId};
use miette::{IntoDiagnostic, Result};
use serde::Serialize;
use tokio::{sync::Mutex, task::JoinSet};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser, Serialize)]
#[command(
    name = "hypha-scheduler",
    version,
    about = "Hypha Scheduler",
    long_about = "Runs the Hypha Scheduler coordinating workers.",
    after_help = "For more information, see the project documentation."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand, Serialize)]
enum Commands {
    Init {
        /// Path where the configuration file will be written
        #[clap(short, long, default_value = "config.toml")]
        output: std::path::PathBuf,
    },
    #[serde(untagged)]
    Run {
        /// Path to the configuration file.
        #[clap(short, long("config"), default_value = "config.toml")]
        #[serde(skip)]
        config_file: std::path::PathBuf,

        /// Addresses of the gateways (can be specified multiple times).
        #[clap(long("gateway"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        gateway_addresses: Option<Vec<Multiaddr>>,

        /// Addresses to listen on (can be specified multiple times).
        #[clap(long("listen"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        listen_addresses: Option<Vec<Multiaddr>>,
    },
}

async fn run(config: ConfigWithMetadata<Config>) -> Result<()> {
    let cert_chain = config.load_cert_chain()?;
    let private_key = config.load_key()?;
    let ca_certs = config.load_trust_chain()?;
    let crls = config.load_crls()?;

    let local_peer_id = identity_from_private_key(&private_key)
        .into_diagnostic()?
        .public()
        .to_peer_id();

    let (network, network_driver) =
        Network::create(cert_chain, private_key, ca_certs, crls).into_diagnostic()?;
    tokio::spawn(network_driver.run());

    join_all(
        config
            .listen_addresses()
            .iter()
            .map(|address| network.listen(address.clone()))
            .collect::<Vec<_>>(),
    )
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .into_diagnostic()?;
    tracing::info!("Successfully listening on all addresses");

    let gateway_peer_ids: Vec<_> = join_all(
        config
            .gateway_addresses()
            .iter()
            .map(|address| network.dial(address.clone()))
            .collect::<Vec<_>>(),
    )
    .await
    .into_iter()
    .filter_map(|result| result.ok())
    .collect();

    if gateway_peer_ids.is_empty() {
        return Err(miette::miette!("Failed to connect to any gateway"));
    }

    tracing::info!(gateway_ids = ?gateway_peer_ids, "Connected to gateway(s)");

    // Wait until DHT bootstrapping is done.
    network.wait_for_bootstrap().await.into_diagnostic()?;

    let scheduler = Arc::new(Mutex::new(Tasks::new()));

    let handler = network
        .on(|req: &hypha_messages::Request| matches!(req, hypha_messages::Request::Worker(_)))
        .into_stream()
        .await
        .into_diagnostic()?;

    let handler_future = {
        let network = network.clone();
        let scheduler = scheduler.clone();

        handler.respond_with_concurrent(None, move |(peer_id, req)| {
            let network = network.clone();
            let scheduler = scheduler.clone();
            async move {
                match req {
                    hypha_messages::Request::Worker(worker) => match worker {
                        hypha_messages::WorkerRequest::Available { task_id, role } => {
                            tracing::debug!(
                                task_id = %task_id,
                                peer_id = %peer_id,
                                "Received available worker request",
                            );
                            let (participants, parameter_server_peer_id) =
                                if let Some(task) = scheduler.lock().await.get_task(&task_id) {
                                    match role {
                                        hypha_messages::WorkerRole::TaskExecutor => {
                                            task.add_worker(peer_id)
                                        }
                                        hypha_messages::WorkerRole::ParameterExecutor => {
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
                            if participants.len() == 2
                                && let Some(parameter_server_peer_id) = parameter_server_peer_id
                            {
                                tokio::spawn(start_task(
                                    network.clone(),
                                    task_id,
                                    parameter_server_peer_id,
                                    participants,
                                ));
                            }

                            hypha_messages::Response::Worker(
                                hypha_messages::WorkerResponse::Available {},
                            )
                        }
                        hypha_messages::WorkerRequest::TaskStatus { task_id, status } => {
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

                            hypha_messages::Response::Worker(
                                hypha_messages::WorkerResponse::TaskStatus {},
                            )
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
        &hypha_messages::RequestAnnounce {
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
                    hypha_messages::Request::Scheduler(hypha_messages::SchedulerRequest::Work {
                        task_id,
                        parameter_server_peer_id,
                    }),
                )
                .await
        });
    }

    let _result = set.join_all().await;
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match &cli.command {
        Commands::Init { output } => {
            fs::write(output, &to_toml(&Config::default())?).into_diagnostic()?;

            println!("Configuration written to: {output:?}");
            Ok(())
        }
        args @ Commands::Run { config_file, .. } => {
            let config: ConfigWithMetadata<Config> = builder()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_SCHEDULER_"))
                .with_provider(Serialized::defaults(args))
                .build()?;

            return run(config).await;
        }
    }
}
