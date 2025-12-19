use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use clap::Parser;
use figment::{
    providers::{Env, Format, Serialized, Toml},
    value::Map,
};
use futures_util::{StreamExt, future::join_all};
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_data::{
    config::Config, hash::get_file_hash, network::Network, tensor_data::serialize_file,
};
use hypha_messages::{DataRecord, data_record, health};
use hypha_network::{
    dial::DialInterface, external_address::ExternalAddressInterface, kad::KademliaInterface,
    listen::ListenInterface, request_response::RequestResponseInterfaceExt,
    stream_pull::StreamPullReceiverInterface, swarm::SwarmDriver,
};
use hypha_telemetry as telemetry;
use libp2p::{Multiaddr, multiaddr::Protocol};
use miette::{Context, IntoDiagnostic, Result};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use serde::{Deserialize, Serialize};
use serde_json::ser::PrettyFormatter;
use tokio::{
    fs,
    io::AsyncWriteExt,
    signal::unix::{SignalKind, signal},
};
use tokio_retry::{
    Retry,
    strategy::{FixedInterval, jitter},
};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    EnvFilter, Layer, Registry, layer::SubscriberExt, util::SubscriberInitExt,
};

#[path = "../cli.rs"]
mod cli;
use cli::{Cli, Commands};

#[derive(Debug, Deserialize, Serialize)]
struct DatasetIndex {
    entries: Vec<DatasetIndexEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct DatasetIndexEntry {
    hash: u64,
    path: PathBuf,
}

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

    // NOTE: Healthy (readiness) is defined as:
    // - listening on all addresses
    // - DHT bootstrap complete
    let ready = Arc::new(AtomicBool::new(false));

    // Load certificates and private key
    let (network, network_driver) = Network::create(
        cert_chain,
        private_key,
        ca_certs,
        crls,
        exclude_cidrs,
        config.network(),
    )
    .into_diagnostic()?;

    let network_handle = tokio::spawn(network_driver.run());

    // Register health handler responding with readiness (listen + DHT bootstrap)
    let ready_clone = ready.clone();
    let health_handle = network
        .on::<health::Codec, _>(|_: &health::Request| true)
        .into_stream()
        .await
        .into_diagnostic()?
        .respond_with_concurrent(None, move |(_, _)| {
            let ready = ready_clone.clone();
            async move {
                let flag = ready.load(Ordering::Relaxed);
                health::Response { healthy: flag }
            }
        });

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

    // NOTE: Add configured external addresses (e.g., public IPs or DNS names)
    // so peers can discover and connect to us.
    join_all(
        config
            .external_addresses()
            .iter()
            .map(|address| network.add_external_address(address.clone()))
            .collect::<Vec<_>>(),
    )
    .await;

    // Dial each gateway and, on success, set up a relay circuit listen via it.
    let gateway_peer_ids = Retry::spawn(
        FixedInterval::from_millis(config.network().rtt_ms().max(100))
            .map(jitter)
            .take(6),
        || {
            let network = network.clone();

            let gateway_addresses = config.gateway_addresses().to_vec();
            let enable_circuit = config.relay_circuit();

            async move {
                let gateway_results = join_all(
                    gateway_addresses
                        .iter()
                        .map(|address| {
                            let address = address.clone();
                            let network = network.clone();
                            async move {
                                let peer_id =
                                    network.dial(address.clone()).await.into_diagnostic()?;
                                // Attempt to setup relay circuit via gateway.
                                if enable_circuit {
                                    if let Ok(relay_addr) = address
                                        .with_p2p(peer_id)
                                        .map(|a| a.with(Protocol::P2pCircuit))
                                    {
                                        network.listen(relay_addr).await.into_diagnostic()?;
                                    } else {
                                        return Err(miette::miette!(
                                            "Failed to construct circuit address"
                                        ));
                                    }
                                }

                                Ok(peer_id)
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
                    tracing::error!("Failed to connect to any gateway");

                    Err(miette::miette!("Failed to connect to any gateway"))
                } else {
                    Ok(gateway_peer_ids)
                }
            }
        },
    )
    .await?;

    tracing::info!(peer_ids = ?gateway_peer_ids, "Connected to gateway(s)");

    // NOTE: Wait until DHT bootstrapping is done.
    network.wait_for_bootstrap().await.into_diagnostic()?;

    // NOTE: Mark data as ready only after listening + bootstrap complete.
    ready.store(true, Ordering::Relaxed);

    // Treat each file in the directory as a subset of the dataset.
    // TODO: determine subset ordering by, e.g., a naming convention of the files in the dataset folder.
    let dataset_path = config.dataset_path();
    if !dataset_path.is_dir() {
        return Err(miette::miette!("Dataset path is not a directory"));
    }

    let dataset_name = match dataset_path.file_name().and_then(|name| name.to_str()) {
        Some(name) => Ok(name.to_string()),
        None => Err(miette::miette!("Dataset path is not a directory")),
    }?;
    let dataset_files = match dataset_path.read_dir() {
        Ok(dir) => Ok(dir
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect::<Vec<_>>()),
        Err(err) => Err(miette::miette!("Failed to read dataset directory: {}", err)),
    }?;

    let index_path = dataset_path.join("index.json");
    let dataset_hashes = if index_path.exists() {
        tracing::info!(
            dataset = %dataset_name,
            index = %index_path.display(),
            "Using dataset index"
        );

        let file = std::fs::File::open(&index_path)
                .into_diagnostic()
                .wrap_err_with(|| format!("Failed to open dataset index at {}. If the file is corrupted, delete it and restart `hypha-data`.", index_path.display()))?;
        let index: DatasetIndex = serde_json::from_reader(file)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!("Failed to parse dataset index at {}", index_path.display())
            })?;
        if index.entries.is_empty() {
            return Err(miette::miette!("Dataset index is empty"));
        }

        let mut hashes = HashMap::with_capacity(index.entries.len());
        for entry in index.entries {
            hashes.insert(entry.hash, dataset_path.join(entry.path));
        }

        hashes
    } else {
        tracing::info!(
            dataset = %dataset_name,
            total_files = dataset_files.len(),
            "Hashing dataset files"
        );

        if dataset_files.is_empty() {
            return Err(miette::miette!("Dataset directory is empty"));
        }

        let dataset_hashes = tokio::task::spawn_blocking(move || {
            // Hash the dataset files
            let start = Instant::now();
            let total_files = dataset_files.len();
            let log_every = std::cmp::max(1, total_files / 10);
            let progress = AtomicUsize::new(0);
            dataset_files
                .par_iter()
                .map(|file| {
                    let hash = get_file_hash(file);
                    let done = progress.fetch_add(1, Ordering::Relaxed) + 1;
                    if done.is_multiple_of(log_every) || done == total_files {
                        tracing::info!(
                            hashed_files = done,
                            total_files,
                            elapsed_ms = start.elapsed().as_millis(),
                            "Hashing dataset files"
                        );
                    }
                    hash.map(|h| (h, file.clone()))
                })
                .collect::<Result<Vec<_>, io::Error>>()
        })
        .await
        .into_diagnostic()?
        .into_diagnostic()?;

        let mut hashes = HashMap::with_capacity(dataset_hashes.len());
        for (hash, path) in dataset_hashes {
            if hashes.insert(hash, path).is_some() {
                return Err(miette::miette!(
                    "Duplicate hash detected while hashing dataset files"
                ));
            }
        }

        let mut entries = Vec::with_capacity(hashes.len());
        for (hash, path) in &hashes {
            let relative = path
                .strip_prefix(dataset_path)
                .into_diagnostic()
                .wrap_err_with(|| {
                    format!(
                        "Failed to write dataset index for path outside dataset root: {}",
                        path.display()
                    )
                })?
                .to_path_buf();
            entries.push(DatasetIndexEntry {
                hash: *hash,
                path: relative,
            });
        }

        let index = DatasetIndex { entries };
        let file = std::fs::File::create_new(&index_path)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!("Failed to create dataset index at {}", index_path.display())
            })?;
        let formatter = PrettyFormatter::with_indent(b"  ");
        let mut serializer = serde_json::Serializer::with_formatter(file, formatter);
        index
            .serialize(&mut serializer)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!("Failed to write dataset index at {}", index_path.display())
            })?;

        tracing::info!(
            dataset = %dataset_name,
            index = %index_path.display(),
            "Wrote dataset index"
        );

        hashes
    };

    // Announce our dataset
    tracing::info!(dataset_name, "Announcing");

    let d = dataset_name.clone();
    let record_handle = network
        .on::<data_record::Codec, _>(move |_: &data_record::Request| true)
        .into_stream()
        .await
        .into_diagnostic()?
        .respond_with_concurrent(None, {
            let dataset_hashes = dataset_hashes.clone();
            move |(_, request)| {
                let d = d.clone();
                let slice_hashes = dataset_hashes.keys().cloned().collect();
                async move {
                    if request.dataset == d {
                        data_record::Response::Success {
                            data_record: DataRecord { slice_hashes },
                        }
                    } else {
                        data_record::Response::NotFound
                    }
                }
            }
        });

    network
        .provide(dataset_name.as_str())
        .await
        .into_diagnostic()?;

    let stream_pulls = network
        .streams_pull()
        .expect("an unregistered pull stream")
        .for_each_concurrent(None, {
            move |request| {
                let dataset_hashes = dataset_hashes.clone();
                let dataset_name = dataset_name.clone();
                async move {
                    let peer_id = request.peer_id();
                    let resource = request.request().clone();
                    tracing::info!(peer_id = %peer_id, dataset = resource.dataset, slice = resource.hash, "Sending tensor to peer");

                    if dataset_name.as_str() != resource.dataset.as_str() {
                        tracing::warn!(peer_id = %peer_id, dataset = resource.dataset, "No dataset found with that name");
                        return;
                    }

                    let Some(dataset_slice) = dataset_hashes.get(&resource.hash) else {
                        tracing::warn!(peer_id = %peer_id, dataset = resource.dataset, "Invalid hash for dataset");
                        return;
                    };

                    let Ok(payload_len) = fs::metadata(dataset_slice).await.map(|m| m.len()) else {
                        tracing::warn!(peer_id = %peer_id, dataset = resource.dataset, "Failed to stat dataset slice");
                        return;
                    };

                    let Ok((_, _, mut stream)) = request.create_response(payload_len).await else {
                        tracing::warn!(peer_id = %peer_id, dataset = resource.dataset, "Failed to announce payload length");
                        return;
                    };

                    if let Err(e) = serialize_file(dataset_slice, &mut stream).await {
                        tracing::warn!(peer_id = %peer_id, dataset = resource.dataset, "Failed to serialize dataset: {}", e);
                    }

                    if let Err(e) = stream.shutdown().await {
                        tracing::warn!(peer_id = %peer_id, dataset = resource.dataset, "Failed to finalize stream: {}", e);
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
        _ = health_handle => {
            tracing::info!("Health handler terminated, shutting down");
        }
        _ = record_handle => {
            tracing::info!("Data record handler terminated, shutting down");
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
                config.network(),
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
