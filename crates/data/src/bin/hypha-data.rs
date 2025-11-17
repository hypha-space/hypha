use std::time::Duration;

use clap::Parser;
use figment::{
    providers::{Env, Format, Serialized, Toml},
    value::Map,
};
use futures_util::{StreamExt, future::join_all};
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_data::{config::Config, network::Network, tensor_data::serialize_file};
use hypha_messages::{DataRecord, health};
use hypha_network::{
    dial::DialInterface, kad::KademliaInterface, listen::ListenInterface,
    request_response::RequestResponseInterfaceExt, stream_pull::StreamPullReceiverInterface,
    swarm::SwarmDriver,
};
use hypha_telemetry as telemetry;
use libp2p::{Multiaddr, kad, multiaddr::Protocol};
use miette::{IntoDiagnostic, Result};
use tokio::{
    fs,
    signal::unix::{SignalKind, signal},
};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    EnvFilter, Layer, Registry, layer::SubscriberExt, util::SubscriberInitExt,
};

#[path = "../cli.rs"]
mod cli;
use cli::{Cli, Commands};

async fn run(config: ConfigWithMetadata<Config>) -> Result<()> {
    let tracing = telemetry::tracing(
        config.telemetry_endpoint(),
        config.telemetry_headers(),
        config.telemetry_protocol(),
        config.telemetry_attributes(),
        config.telemetry_sampler(),
        config.telemetry_sample_ratio(),
    )
    .into_diagnostic()?;

    let logging = telemetry::logging(
        config.telemetry_endpoint(),
        config.telemetry_headers(),
        config.telemetry_protocol(),
        config.telemetry_attributes(),
    )
    .into_diagnostic()?;

    let metrics = telemetry::metrics(
        config.telemetry_endpoint(),
        config.telemetry_headers(),
        config.telemetry_protocol(),
        config.telemetry_attributes(),
        std::time::Duration::from_secs(1),
    )
    .into_diagnostic()?;

    telemetry::metrics::global::set_provider(metrics.provider());

    Registry::default()
        .with(
            tracing_subscriber::fmt::layer().with_filter(
                EnvFilter::builder()
                    .with_default_directive(LevelFilter::INFO.into())
                    .from_env_lossy(),
            ),
        )
        .with(tracing.layer())
        .with(logging.layer())
        .init();

    let cert_chain = config.load_cert_chain()?;
    let private_key = config.load_key()?;
    let ca_certs = config.load_trust_chain()?;
    let crls = config.load_crls()?;

    let exclude_cidrs = config.exclude_cidr().clone();

    // Load certificates and private key
    let (network, network_driver) =
        Network::create(cert_chain, private_key, ca_certs, crls, exclude_cidrs)
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
    tracing::info!(dataset_name, "Announcing");
    let _ = network
        .store(kad::Record::new(
            kad::RecordKey::new(&dataset_name),
            serde_json::to_vec(&DataRecord {
                num_slices: dataset_files.len() as u64,
            })
            .map_err(|err| miette::miette!("Failed to serialize dataset record: {}", err))?,
        ))
        .await;

    let stream_pulls = network.streams_pull().expect("an unregistered pull stream").for_each_concurrent(None, {
        |(peer_id, resource, mut stream)| {
            let dataset_files = dataset_files.clone();
            async move {
                tracing::info!(peer_id = %peer_id, dataset = resource.dataset, slice= resource.index, "Sending tensor to peer");

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
    let cli = Cli::parse();
    match &cli.command {
        args @ Commands::Init {
            output,
            name,
            dataset_path,
            ..
        } => {
            let mut config_builder =
                builder::<Config>().with_provider(Serialized::defaults(&Config::default()));

            // Override config fields if values are provided.
            if let Some(name) = name {
                config_builder = config_builder.with_provider(Serialized::defaults(Map::from([
                    ("cert_pem", format!("{name}-cert.pem")),
                    ("key_pem", format!("{name}-key.pem")),
                    ("trust_pem", format!("{name}-trust.pem")),
                ])));
            }
            if let Some(dataset_path) = dataset_path {
                config_builder = config_builder
                    .with_provider(Serialized::default("dataset_path", dataset_path.clone()));
            }

            let config = config_builder
                .with_provider(Serialized::defaults(&args))
                .build()?
                .validate()?;

            fs::write(output, &to_toml(&config.config).into_diagnostic()?)
                .await
                .into_diagnostic()?;

            println!("Configuration written to: {output:?}");
            Ok(())
        }
        args @ Commands::Probe {
            config_file,
            address,
            timeout,
            ..
        } => {
            let config: ConfigWithMetadata<Config> = builder::<Config>()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_"))
                .with_provider(Serialized::from(&args, figment::Profile::Default))
                .build()?
                .validate()?;

            let exclude_cidrs = config.exclude_cidr().clone();
            let (network, driver) = Network::create(
                config.load_cert_chain()?,
                config.load_key()?,
                config.load_trust_chain()?,
                config.load_crls()?,
                exclude_cidrs,
            )
            .into_diagnostic()?;
            tokio::spawn(driver.run());

            let addr: Multiaddr = address.parse().into_diagnostic()?;
            tokio::time::timeout(Duration::from_millis(*timeout), async move {
                let peer = network.dial(addr).await.into_diagnostic()?;
                let resp = network
                    .request::<health::Codec>(peer, health::Request {})
                    .await
                    .into_diagnostic()?;
                if resp.healthy {
                    Ok(())
                } else {
                    Err(miette::miette!("unhealthy"))
                }
            })
            .await
            .into_diagnostic()??;

            Ok(())
        }
        args @ Commands::Run { config_file, .. } => {
            let config: ConfigWithMetadata<Config> = builder::<Config>()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_"))
                .with_provider(Env::prefixed("OTEL_"))
                .with_provider(Serialized::defaults(args))
                .build()?
                .validate()?;

            run(config).await
        }
    }
}
