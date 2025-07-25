//! Worker binary.

use std::{error::Error, fs, net::SocketAddr, path::PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use figment::providers::{Env, Format, Serialized, Toml};
use hypha_config::LayeredConfig;
use hypha_network::{
    dial::DialInterface,
    kad::KademliaInterface,
    listen::ListenInterface,
    swarm::SwarmDriver,
    utils::{multiaddr_from_socketaddr, multiaddr_from_socketaddr_quic},
};
use hypha_worker::{
    arbiter::Arbiter, config::Config, connector::Connector, job_manager::JobManager,
    lease_manager::ResourceLeaseManager, network::Network,
    request_evaluator::WeightedResourceRequestEvaluator, resources::StaticResourceManager,
};
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
    long_about = "Runs a Hypha Worker which executes jobs.",
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

        /// Address of the gateway.
        #[clap(long("gateway"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        gateway_address: Option<SocketAddr>,

        /// Address to listen on.
        #[clap(long("listen"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        listen_address: Option<SocketAddr>,

        /// Socket to use for driver communication.
        #[clap(long("socket"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        socket_address: Option<PathBuf>,
    },
}

async fn run(config: Config) -> Result<(), Box<dyn Error>> {
    // Load certificates and private key
    let cert_chain = config.load_cert_chain()?;
    let private_key = config.load_key()?;
    let ca_certs = config.load_trust_chain()?;
    let crls = config.load_crls()?;

    let (network, network_driver) = Network::create(cert_chain, private_key, ca_certs, crls)?;
    let network_handle = tokio::spawn(network_driver.run());

    network
        .listen(multiaddr_from_socketaddr(config.listen_address()).into_diagnostic()?)
        .await?;
    tracing::info!("Successfully listening");

    // Dial the gateway address
    // TODO: fall back to TCP if QUIC doesn't work
    let _gateway_peer_id = network
        .dial(multiaddr_from_socketaddr_quic(config.gateway_address()).into_diagnostic()?)
        .await
        .into_diagnostic()?;

    tracing::info!(gateway_id = %_gateway_peer_id, "Connected to gateway");

    // Wait until DHT bootstrapping is done.
    network.wait_for_bootstrap().await?;

    let token = CancellationToken::new();

    // NOTE: Create arbiter for resource allocation - this is the primary worker allocation mechanism
    // Initialize components from configuration
    // let (ledger, ledger_rx) = ResourceLedger::new();
    // let resource_observer = ResourceObserver::with_config(config.observer.clone());
    //
    let arbiter = Arbiter::new(
        ResourceLeaseManager::new(StaticResourceManager::new(config.resources())),
        WeightedResourceRequestEvaluator::default(),
        network.clone(),
    )
    .with_job_manager(JobManager::new(Connector::new(network.clone())));

    let arbiter_handle = tokio::spawn(async move {
        if let Err(e) = arbiter.run().await {
            tracing::error!(error = %e, "Arbiter failed");
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
        _ = network_handle => {
            tracing::warn!("Network driver error, shutting down");
        }
        _ = arbiter_handle => {
            tracing::warn!("Arbiter error, shutting down");
        }
    }

    token.cancel();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match &cli.command {
        Commands::Init { output } => {
            fs::write(output, &Config::default().to_toml().into_diagnostic()?).into_diagnostic()?;

            println!("Configuration written to: {output:?}");
            Ok(())
        }
        args @ Commands::Run { config_file, .. } => {
            let config = Config::builder()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_"))
                .with_provider(Serialized::defaults(args))
                .build()?;

            run(config).await
        }
    }
}
