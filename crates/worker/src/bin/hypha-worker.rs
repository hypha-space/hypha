//! Worker binary.

use std::{fs, path::PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use figment::providers::{Env, Format, Serialized, Toml};
use futures_util::{StreamExt, future::join_all};
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_network::{
    dial::DialInterface,
    gossipsub::GossipsubInterface,
    kad::KademliaInterface,
    listen::ListenInterface,
    request_response::{RequestResponseInterface, RequestResponseInterfaceExt},
    stream::StreamReceiverInterface,
    swarm::SwarmDriver,
};
use hypha_worker::{config::Config, driver, file_transfer::receive_file, network::Network};
use libp2p::Multiaddr;
use miette::{IntoDiagnostic, Result};
use serde::{Deserialize, Serialize};
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Clone, Debug, ValueEnum, Serialize, Deserialize)]
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
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand, Serialize)]
enum Commands {
    Init {
        /// Path where the configuration file will be written
        #[clap(short, long, default_value = "config.toml")]
        output: PathBuf,
    },
    #[serde(untagged)]
    Run {
        /// Path to the configuration file.
        #[clap(short, long("config"), default_value = "config.toml")]
        #[serde(skip)]
        config_file: PathBuf,

        /// Addresses of the gateways (can be specified multiple times).
        #[clap(long("gateway"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        gateway_addresses: Option<Vec<Multiaddr>>,

        /// Addresses to listen on (can be specified multiple times).
        #[clap(long("listen"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        listen_addresses: Option<Vec<Multiaddr>>,

        /// Socket to use for driver communication.
        #[clap(long("socket"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        socket_address: Option<PathBuf>,

        // NOTE: Defining the role via command-line argument is a workaround used to determine the worker's role in DiLoCo tasks. The role will be assigned by the scheduler in the future.
        /// Role of the worker in DiLoCo tasks.
        #[arg(value_enum)]
        #[serde(skip)]
        role: Role,
    },
}

async fn run(config: ConfigWithMetadata<Config>, role: Role) -> Result<()> {
    // Load certificates and private key
    let (network, network_driver) = Network::create(
        config.load_cert_chain()?,
        config.load_key()?,
        config.load_trust_chain()?,
        config.load_crls()?,
    )
    .into_diagnostic()?;

    let driver_future = tokio::spawn(network_driver.run());

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

    let token = CancellationToken::new();

    let work_dir = tempfile::tempdir().into_diagnostic()?;

    let driver = driver::try_new_accelerate(
        network.clone(),
        config.socket_path(),
        work_dir.path(),
        token.clone(),
    )
    .await
    .into_diagnostic()?;

    let handler = network
        .on(|req: &hypha_messages::Request| matches!(req, hypha_messages::Request::Scheduler(_)))
        .into_stream()
        .await
        .into_diagnostic()?;

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

    let mut tensor_stream = network.streams().into_diagnostic()?;

    let stream_future = tokio::spawn(async move {
        while let Some((peer_id, mut stream)) = tensor_stream.next().await {
            tracing::debug!(peer_id = %peer_id, "Received tensor stream");

            if let Err(e) = receive_file(work_dir.path(), &mut stream).await {
                tracing::error!(error = ?e, "Failed to receive file");
            }
        }
    });

    let mut announce_stream = network.subscribe("announce").await.into_diagnostic()?;

    let announce_future = tokio::spawn(async move {
        while let Some(Ok(message)) = announce_stream.next().await {
            let announce: hypha_messages::RequestAnnounce =
                match ciborium::from_reader(message.data.as_slice()) {
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
            let role = match role {
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

    let mut sigterm = signal(SignalKind::terminate()).into_diagnostic()?;

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

#[tokio::main]
async fn main() -> miette::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match &cli.command {
        Commands::Init { output } => {
            fs::write(output, &to_toml(&Config::default()).into_diagnostic()?).into_diagnostic()?;

            println!("Configuration written to: {output:?}");
            Ok(())
        }
        args @ Commands::Run {
            config_file, role, ..
        } => {
            let config: ConfigWithMetadata<Config> = builder()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_"))
                .with_provider(Serialized::defaults(args))
                .build()?;

            run(config, role.clone()).await
        }
    }
}
