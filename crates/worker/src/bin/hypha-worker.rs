//! Worker binary.

use std::{fs, path::PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use figment::providers::{Env, Format, Serialized, Toml};
use futures_util::future::join_all;
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_network::{
    dial::DialInterface, kad::KademliaInterface, listen::ListenInterface, swarm::SwarmDriver,
};
use hypha_worker::{
    arbiter::Arbiter, config::Config, connector::Connector, job_manager::JobManager,
    lease_manager::ResourceLeaseManager, network::Network,
    request_evaluator::WeightedResourceRequestEvaluator, resources::StaticResourceManager,
};
use libp2p::{Multiaddr, multiaddr::Protocol};
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
    },
}

async fn run(config: ConfigWithMetadata<Config>) -> Result<()> {
    // Load certificates and private key
    let (network, network_driver) = Network::create(
        config.load_cert_chain()?,
        config.load_key()?,
        config.load_trust_chain()?,
        config.load_crls()?,
    )
    .into_diagnostic()?;

    let network_handle = tokio::spawn(network_driver.run());

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

    // Dial each gateway and, on success, set up a relay circuit listen via it.
    let gateway_results = join_all(
        config
            .gateway_addresses()
            .iter()
            .map(|address| {
                let address = address.clone();
                let network = network.clone();
                async move {
                    match network.dial(address.clone()).await {
                        Ok(peer_id) => {
                            match address
                                .with_p2p(peer_id)
                                .map(|a| a.with(Protocol::P2pCircuit))
                            {
                                Ok(relay_addr) => {
                                    if let Err(e) = network.listen(relay_addr).await {
                                        tracing::warn!(error=%e, "Failed to set up P2pCircuit listen via gateway");
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(error=%e, "Failed to construct relay listen address");
                                }
                            }

                            Ok(peer_id)
                        }
                        Err(e) => Err(e),
                    }
                }
            })
            .collect::<Vec<_>>(),
    )
    .await;

    let gateway_peer_ids: Vec<_> = gateway_results
        .into_iter()
        .filter_map(|result| result.ok())
        .collect();

    if gateway_peer_ids.is_empty() {
        return Err(miette::miette!("Failed to connect to any gateway"));
    }

    tracing::info!(gateway_ids = ?gateway_peer_ids, "Connected to gateway(s)");

    // NOTE: Wait until DHT bootstrapping is done.
    network.wait_for_bootstrap().await.into_diagnostic()?;

    let token = CancellationToken::new();

    // NOTE: Create arbiter for resource allocation - this is the primary worker allocation mechanism
    let arbiter = Arbiter::new(
        ResourceLeaseManager::new(StaticResourceManager::new(
            config.resources(),
            config.driver(),
        )),
        WeightedResourceRequestEvaluator::default(),
        network.clone(),
        JobManager::new(Connector::new(network.clone())),
        token.clone(),
    );

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
    }

    token.cancel();

    // Wait for the arbiter to shut down gracefully.
    let _ = arbiter_handle.await;

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
        args @ Commands::Run { config_file, .. } => {
            let config: ConfigWithMetadata<Config> = builder()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_"))
                .with_provider(Serialized::defaults(args))
                .build()?;

            run(config).await
        }
    }
}
