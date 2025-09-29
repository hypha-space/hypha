use std::path::PathBuf;

use clap::{Parser, Subcommand};
use figment::providers::{Env, Format, Serialized, Toml};
use futures_util::{StreamExt, future::join_all};
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_data::{config::Config, network::Network, tensor_data::serialize_file};
use hypha_network::{
    dial::DialInterface, kad::KademliaInterface, listen::ListenInterface,
    stream_pull::StreamPullReceiverInterface, swarm::SwarmDriver,
};
use libp2p::multiaddr::Protocol;
use miette::{IntoDiagnostic, Result};
use serde::Serialize;
use tokio::{
    fs,
    signal::unix::{SignalKind, signal},
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "hypha-data",
    version,
    about = "Hypha Data Node",
    long_about = "Runs a Hypha Data Node which provides data.",
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

    // Dial each gateway and, optionally, set up a relay circuit listen via it.
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
                            // NOTE: When enabled, listen via the gateway relay circuit for inbound reachability.
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

    // Treat each file in the directory as a subset of the dataset.
    // TODO: determine subset ordering by, e.g., a naming convention of the files in the dataset folder.
    let dataset_path = config.dataset_path();
    if !dataset_path.is_dir() {
        return Err(miette::miette!("Dataset path is not a directory"));
    }

    let dataset_name = match dataset_path.file_name().and_then(|name| name.to_str()) {
        Some(name) => Ok(name),
        None => Err(miette::miette!("Dataset path is not a directory")),
    }?;
    let dataset_files = match dataset_path.read_dir() {
        Ok(dir) => Ok(dir
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect::<Vec<_>>()),
        Err(err) => Err(miette::miette!("Failed to read dataset directory: {}", err)),
    }?;

    if dataset_files.is_empty() {
        return Err(miette::miette!("Dataset directory is empty"));
    }

    // Announce our dataset
    // TODO: We need to announce the number of subsets of this dataset as well.
    // This is needed for schedulers to figure out which subsets to assign to workers in data-parallel training.
    tracing::info!(dataset_name, "Announcing");
    let _ = network.provide(dataset_name).await;

    let stream_pulls = network.streams_pull().expect("an unregistered pull stream").for_each_concurrent(None, {
        |(peer_id, resource, mut stream)| {
            let dataset_files = dataset_files.clone();
            async move {
                tracing::info!(peer_id = %peer_id, dataset = resource.dataset, "Sending tensor to peer");

                if dataset_name == resource.dataset.as_str() {
                    // Use the provided offsets to select a subset of the available dataset files.
                    if resource.index as usize > dataset_files.len() {
                        tracing::warn!(peer_id = %peer_id, dataset = resource.dataset, "Invalid index for dataset");
                        return;
                    }
                    let dataset_slice = &dataset_files[resource.index as usize];

                    if let Err(e) = serialize_file(dataset_slice, &mut stream).await {
                        tracing::warn!(peer_id = %peer_id, dataset = resource.dataset, "Failed to serialize dataset: {}", e);
                    }
                } else {
                    tracing::warn!(peer_id = %peer_id, dataset = resource.dataset, "No dataset found with that name");
                }
            }
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
        _ = stream_pulls => {
            tracing::warn!("Stream pull error, shutting down");
        }
    }

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
            fs::write(output, &to_toml(&Config::default()).into_diagnostic()?)
                .await
                .into_diagnostic()?;

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
