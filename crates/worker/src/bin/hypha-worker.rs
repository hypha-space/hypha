//! Worker binary.

use std::{error::Error, fs, net::SocketAddr, path::PathBuf, time::Duration};

use clap::{Parser, ValueEnum};
use futures_util::StreamExt;
use hypha_network::{
    cert::{load_certs_from_pem, load_crls_from_pem, load_private_key_from_pem},
    dial::DialInterface,
    gossipsub::GossipsubInterface,
    listen::ListenInterface,
    request_response::{RequestResponseInterface, RequestResponseInterfaceExt},
    stream::StreamReceiverInterface,
    swarm::SwarmDriver,
    utils::multiaddr_from_socketaddr,
};
use hypha_worker::{driver, file_transfer::receive_file, network::Network};
use libp2p::multiaddr::Protocol;
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

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
    /// Socket to use for driver communication.
    #[clap(long, default_value = "/tmp/hypha.sock")]
    socket: PathBuf,
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
    let driver_future = tokio::spawn(network_driver.run());

    network
        .listen(multiaddr_from_socketaddr(opt.listen_address)?)
        .await?;
    tracing::info!("Successfully listening");

    // Dial the gateway address
    let gateway_peer_id = network.dial(gateway_address.clone()).await?;

    network
        .listen(
            gateway_address
                .with_p2p(gateway_peer_id)
                .unwrap()
                .with(Protocol::P2pCircuit),
        )
        .await?;

    tracing::info!(gateway_id = %gateway_peer_id, "Connected to gateway");
    // Wait a bit until DHT bootstrapping is done.
    // Once we receive an 'Identify' message, bootstrapping will start.
    // TODO: Provide a way to wait for this event
    tokio::time::sleep(Duration::from_secs(2)).await;

    let token = CancellationToken::new();

    let work_dir = tempfile::tempdir()?;

    let driver = driver::try_new_accelerate(
        network.clone(),
        opt.socket.as_path(),
        work_dir.path(),
        token.clone(),
    )
    .await?;

    let handler = network
        .on(|req: &hypha_messages::Request| matches!(req, hypha_messages::Request::Scheduler(_)))
        .into_stream()
        .await?;

    let handler_future = {
        let driver = driver.clone();
        handler.respond_with_concurrent(None, move |(peer_id, req)| {
            let mut driver = driver.clone();
            async move {
                match req {
                    hypha_messages::Request::Scheduler(scheduler) => match scheduler {
                        hypha_messages::SchedulerRequest::Work {
                            task_id,
                            parameter_server_peer_id,
                        } => {
                            tracing::info!(
                                task_id = %task_id,
                                peer_id = %peer_id,
                                "Received work request"
                            );

                            driver
                                .start_training(peer_id, parameter_server_peer_id, task_id)
                                .await;

                            hypha_messages::Response::Scheduler(
                                hypha_messages::SchedulerResponse::Work {},
                            )
                        }
                    },
                    _ => panic!("Unexpected request received"),
                }
            }
        })
    };

    let mut tensor_stream = network.streams()?;

    let stream_future = tokio::spawn(async move {
        while let Some((peer_id, mut stream)) = tensor_stream.next().await {
            tracing::info!(peer_id = %peer_id, "Received tensor stream");

            if let Err(e) = receive_file(work_dir.path(), &mut stream).await {
                tracing::error!(error = ?e, "Failed to receive file");
            }
        }
    });

    let mut announce_stream = network.subscribe("announce").await?;

    let announce_future = tokio::spawn(async move {
        while let Some(Ok(data)) = announce_stream.next().await {
            let announce: hypha_messages::RequestAnnounce =
                match ciborium::from_reader(data.as_slice()) {
                    Ok(announce) => announce,
                    Err(e) => {
                        tracing::error!(error = ?e, "Failed to deserialize announce message");
                        continue;
                    }
                };

            tracing::debug!(
                peer_id = %announce.scheduler_peer_id,
                "Sending availability to scheduler"
            );
            let role = match opt.role {
                Role::Worker => hypha_messages::WorkerRole::TaskExecutor,
                Role::ParameterServer => hypha_messages::WorkerRole::ParameterExecutor,
            };
            let _ = network
                .request(
                    announce.scheduler_peer_id,
                    hypha_messages::Request::Worker(hypha_messages::WorkerRequest::Available {
                        task_id: announce.task_id,
                        role,
                    }),
                )
                .await;
        }
    });

    let mut sigterm = signal(SignalKind::terminate())?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received SIGINT, shutting down");
        }
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM, shutting down");
        }
        // All of these futures handle streams of events.
        // If any of them terminate on their own, it must be due to an error.
        // We warn about that, then shut down gracefully.
        // TODO: Log errors if futures return one.
        _ = driver_future => {
            tracing::warn!("Network driver error, shutting down");
        }
        _ = handler_future => {
            tracing::warn!("Network driver error, shutting down");
        }
        _ = announce_future => {
            tracing::warn!("Network driver error, shutting down");
        }
        _ = stream_future => {
            tracing::warn!("Network driver error, shutting down");
        }
    }

    token.cancel();
    driver.wait().await;

    Ok(())
}
