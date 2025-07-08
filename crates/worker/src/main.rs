mod network;

use std::{error::Error, fs, net::SocketAddr, path::PathBuf, time::Duration};

use clap::{Parser, ValueEnum};
use futures_util::StreamExt;
use hypha_network::{
    cert::{load_certs_from_pem, load_crls_from_pem, load_private_key_from_pem},
    dial::DialInterface,
    gossipsub::GossipsubInterface,
    listen::ListenInterface,
    request_response::{RequestResponseInterface, RequestResponseInterfaceExt},
    swarm::SwarmDriver,
    utils::multiaddr_from_socketaddr,
};
use tracing_subscriber::EnvFilter;

use crate::network::Network;

#[derive(Clone, Debug, ValueEnum)]
enum Role {
    Worker,
    ParameterServer,
}

#[derive(Debug, Parser)]
#[command(
    name = "hypha-worker",
    version,
    about = "Hypha Worker Node",
    long_about = "Runs a Hypha Worker which executes tasks.",
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
    /// Role of the worker in DiLoCo tasks.
    #[arg(value_enum)]
    role: Role,
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

    let gateway_address = multiaddr_from_socketaddr(opt.gateway_address)?;

    let (network, network_driver) = Network::create(cert_chain, private_key, ca_certs, crls)?;
    tokio::spawn(network_driver.run());

    network
        .listen(multiaddr_from_socketaddr(opt.listen_address)?)
        .await?;
    tracing::info!("Successfully listening");

    // Dial the gateway address
    let _gateway_peer_id = network.dial(gateway_address).await?;

    tracing::info!(gateway_id = %_gateway_peer_id, "Connected to gateway");
    // Wait a bit until DHT bootstrapping is done.
    // Once we receive an 'Identify' message, bootstrapping will start.
    // TODO: Provide a way to wait for this event
    tokio::time::sleep(Duration::from_secs(2)).await;

    let handler = network
        .on(|req: &hypha_api::Request| matches!(req, hypha_api::Request::Scheduler(_)))
        .into_stream()
        .await?;

    let handler_future = handler.respond_with_concurrent(None, |(peer_id, req)| {
        async move {
            match req {
                hypha_api::Request::Scheduler(scheduler) => match scheduler {
                    hypha_api::SchedulerRequest::Work { task_id, .. } => {
                        tracing::debug!(
                            task_id = %task_id,
                            peer_id = %peer_id,
                            "Received work request"
                        );
                        // TODO: Implement the logic to handle the work request
                        // ...
                        hypha_api::Response::Scheduler(hypha_api::SchedulerResponse::Work {})
                    }
                },
                _ => {
                    panic!("Unexpected request received");
                }
            }
        }
    });

    let mut announce_stream = network.subscribe("announce").await?;

    let announce_future = tokio::spawn(async move {
        while let Some(Ok(data)) = announce_stream.next().await {
            let announce: hypha_api::RequestAnnounce =
                ciborium::from_reader(data.as_slice()).unwrap();

            tracing::debug!(
                peer_id = %announce.scheduler_peer_id,
                "Sending availability to scheduler"
            );
            let role = match opt.role {
                Role::Worker => hypha_api::WorkerRole::TaskExecutor,
                Role::ParameterServer => hypha_api::WorkerRole::ParameterExecutor,
            };
            let _ = network
                .request(
                    announce.scheduler_peer_id,
                    hypha_api::Request::Worker(hypha_api::WorkerRequest::Available {
                        task_id: announce.task_id,
                        role,
                    }),
                )
                .await;
        }
    });

    tokio::select! {
        _ = handler_future => {}
        _ = announce_future => {}
    }

    Ok(())
}
