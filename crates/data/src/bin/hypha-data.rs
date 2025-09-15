use std::path::PathBuf;

use clap::{Parser, Subcommand};
use figment::providers::{Env, Format, Serialized, Toml};
use futures_util::{StreamExt, future::join_all};
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_data::{config::Config, network::Network, tensor};
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

    let tensor_file = config.tensor_file().clone();
    let tensor_name = tensor_file
        .file_stem()
        .expect("a file name")
        .to_str()
        .expect("a valid UTF8 string");

    // Announce our dataset
    tracing::info!(tensor_name, "Announcing");
    let _ = network.provide(tensor_name).await;

    let stream_pulls = network.streams_pull().expect("an unregistered pull stream").for_each_concurrent(None, {
        |(peer_id, resource, mut stream)| {
            let tensor_file = tensor_file.clone();
            async move {
                tracing::info!(peer_id = %peer_id, resource, "Sending tensor to peer");

                if tensor_name == resource.as_str() {
                    if let Ok(tensors) = tensor::TensorData::try_open(tensor_file.as_path()) {
                        let metadata = tensors.metadata().unwrap();
                        tracing::info!(metadata = ?metadata, "Tensor metadata");

                        // TODO: determine offsets
                        let slice = tensors.slice((0, 6)).unwrap();

                        tensor::serialize(slice, &None, &mut stream).await.unwrap();
                    } else {
                        tracing::warn!(peer_id = %peer_id, resource, "Failed to open tensor file");
                    }
                } else {
                    tracing::warn!(peer_id = %peer_id, resource, "No tensor found with that name");
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
