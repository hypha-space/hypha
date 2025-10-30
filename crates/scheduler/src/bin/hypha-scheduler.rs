//! Scheduler binary.

use std::{fs, path::PathBuf, time::Duration};

use clap::{Parser, Subcommand};
use figment::providers::{Env, Format, Serialized, Toml};
use futures_util::future::join_all;
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_messages::{
    DataRecord, Executor, Fetch, JobSpec, Receive, SelectionStrategy, Send, health,
};
use hypha_network::{
    IpNet, cert::identity_from_private_key, dial::DialInterface,
    external_address::ExternalAddressInterface, kad::KademliaInterface, listen::ListenInterface,
    request_response::RequestResponseInterfaceExt, swarm::SwarmDriver,
};
use hypha_scheduler::{
    allocator::{Allocator, GreedyWorkerAllocator},
    config::Config,
    network::Network,
    scheduler_config::{DiLoCo, SchedulerConfig},
    slice_tracker::SliceTracker,
    status_bridge::{AimConnector, NoOpConnector, StatusBridge},
    task::Task,
    worker::Worker,
};
use hypha_telemetry as telemetry;
use libp2p::{Multiaddr, PeerId, multiaddr::Protocol};
use miette::{IntoDiagnostic, Result};
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{
    EnvFilter, Layer, Registry, layer::SubscriberExt, util::SubscriberInitExt,
};

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
    /// Probe a target multiaddr for readiness and exit 0 if healthy.
    #[serde(untagged)]
    Probe {
        /// Path to the configuration file.
        #[clap(short, long("config"), default_value = "config.toml")]
        config_file: PathBuf,

        /// Path to the certificate pem.
        #[clap(long("cert"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        cert_pem: Option<PathBuf>,

        /// Path to the private key pem.
        #[clap(long("key"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        key_pem: Option<PathBuf>,

        /// Path to the trust pem (bundle).
        #[clap(long("trust"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        trust_pem: Option<PathBuf>,

        /// Path to the certificate revocation list pem.
        #[clap(long("crls"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        crls_pem: Option<PathBuf>,

        /// Timeout in milliseconds
        #[clap(long, default_value_t = 2000)]
        timeout: u64,

        /// Target multiaddr to probe (e.g., /ip4/127.0.0.1/tcp/8080)
        #[clap(index = 1)]
        address: String,
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

        /// External addresses to advertise (can be specified multiple times).
        #[clap(long("external"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        external_addresses: Option<Vec<Multiaddr>>,

        /// Enable listening via relay P2pCircuit (via gateway). Defaults to true.
        #[clap(long("relay-circuit"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        relay_circuit: Option<bool>,

        /// CIDR exclusion (repeatable). Overrides config if provided.
        /// Example: --exclude-cidr 10.0.0.0/8 --exclude-cidr fc00::/7
        #[clap(long("exclude-cidr"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        exclude_cidr: Option<Vec<IpNet>>,
    },
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
        .with(tracing_subscriber::fmt::layer().with_filter(EnvFilter::from_default_env()))
        .with(tracing.layer())
        .with(logging.layer())
        .init();

    let cert_chain = config.load_cert_chain()?;
    let private_key = config.load_key()?;
    let ca_certs = config.load_trust_chain()?;
    let crls = config.load_crls()?;

    let peer_id = identity_from_private_key(&private_key)
        .expect("a valid private key")
        .public()
        .to_peer_id();

    let exclude_cidrs = config.exclude_cidr().clone();

    let (network, network_driver) =
        Network::create(cert_chain, private_key, ca_certs, crls, exclude_cidrs)
            .into_diagnostic()?;
    let mut driver_task = tokio::spawn(network_driver.run());

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

    // NOTE: Add configured external addresses so peers can discover and connect.
    join_all(
        config
            .external_addresses()
            .iter()
            .map(|address| network.add_external_address(address.clone()))
            .collect::<Vec<_>>(),
    )
    .await;

    // Dial each gateway and, on success, set up a relay circuit listen via it.
    let gateway_results = join_all(
        config
            .gateway_addresses()
            .iter()
            .map(|address| {
                let address = address.clone();
                let network = network.clone();
                let enable_circuit = config.relay_circuit();
                async move {
                    match network.dial(address.clone()).await {
                        Ok(peer_id) => {
                            if enable_circuit {
                                // Attempt to listen on the relay circuit via the connected gateway.
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
                            } else {
                                tracing::info!("Relay circuit listening disabled; skipping P2pCircuit listen setup");
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

    // Wait until DHT bootstrapping is done.
    network.wait_for_bootstrap().await.into_diagnostic()?;

    let token = CancellationToken::new();

    let SchedulerConfig::DiLoCo(diloco_config) = config.scheduler_config();

    // NOTE: Create allocator for resource allocation alongside existing job management
    let allocator = GreedyWorkerAllocator::new(network.clone());

    tracing::info!("Starting worker allocation and job creation process");

    let worker_spec = hypha_messages::WorkerSpec {
        requirements: diloco_config.resources.worker.clone(),
    };

    let parameter_server_spec = hypha_messages::WorkerSpec {
        requirements: diloco_config.resources.parameter_server.clone(),
    };

    // NOTE: Phase 1 - Allocate workers using the new allocator/arbiter protocol
    // First, request worker nodes for job execution
    let allocated_workers = match allocator
        .request(
            worker_spec.clone(),
            100.0,
            None,
            diloco_config.resources.num_workers as usize,
        )
        .await
    {
        Ok(allocated_workers) => {
            tracing::info!(
                num_workers = %allocated_workers.len(),
                "Successfully allocated workers"
            );
            allocated_workers
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to allocate worker");

            Vec::new()
        }
    };

    // Request multiple workers to increase chances of two distinct peers
    let allocated_parameter_servers = match allocator
        .request(parameter_server_spec.clone(), 100.0, None, 1)
        .await
    {
        Ok(allocated_parameter_servers) => {
            tracing::info!(
                num_parameter_servers = %allocated_parameter_servers.len(),
                "Successfully allocated parameter servers"
            );

            allocated_parameter_servers
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to allocate worker");

            Vec::new()
        }
    };

    let dataset = diloco_config.dataset.dataset.clone();
    let (data_provider, dataset_record) = get_data_provider(&network, dataset.as_str()).await?;

    let slice_tracker = SliceTracker::new(
        network.clone(),
        data_provider,
        dataset.clone(),
        dataset_record.num_slices,
    );

    let tracker_task = tokio::spawn(slice_tracker.run().await.expect("network ready"));

    let status_handle = if allocated_workers.len() >= diloco_config.resources.num_workers as usize
        && allocated_parameter_servers.len() == 1
    {
        let worker1 = Worker::create(allocated_workers[0].clone(), network.clone()).await;

        let worker2 = Worker::create(allocated_workers[1].clone(), network.clone()).await;

        let worker_ids = allocated_workers
            .iter()
            .map(|w| w.peer_id)
            .collect::<Vec<_>>();

        let parameter_server =
            Worker::create(allocated_parameter_servers[0].clone(), network.clone()).await;

        let worker_task = Task::try_new(
            network.clone(),
            JobSpec {
                executor: get_executor_with_dataset(
                    diloco_config,
                    peer_id,
                    dataset.clone(),
                    parameter_server.peer_id(),
                ),
            },
            &[&worker1, &worker2],
        )
        .await
        .into_diagnostic()?;

        let parameter_server_task = Task::try_new(
            network.clone(),
            JobSpec {
                executor: Executor::ParameterServer {
                    updates: Receive::peers(worker_ids.clone()),
                    results: Send::peers(worker_ids.clone(), SelectionStrategy::All),
                    optimizer: diloco_config.parameter_server.optimizer.clone(),
                },
            },
            &[&parameter_server],
        )
        .await
        .into_diagnostic()?;

        let mut status_bridge = match config.status_bridge() {
            Some(value) => StatusBridge::new(Box::new(AimConnector::new(value))),
            None => StatusBridge::new(Box::new(NoOpConnector::new())),
        };

        status_bridge.register_stream(worker_task);
        status_bridge.register_stream(parameter_server_task);

        let cancel_token = token.clone();
        tokio::spawn(async move {
            tokio::select! {
                Err(e) = status_bridge.run(cancel_token) => {
                    tracing::error!(error = %e, "Status bridge failed");
                }
                Err(e) = worker1 => {
                    tracing::error!(error = %e, "Worker 1 failed");
                },
                Err(e) = worker2 => {
                    tracing::error!(error = %e, "Worker 2 failed");
                }
                Err(e) = parameter_server => {
                    tracing::error!(error = %e, "Parameter Server failed");
                }
            }
        })
    } else {
        tracing::warn!("Insufficient workers allocated for diloco training");
        tokio::spawn(async move {})
    };

    // Wait for Ctrl-C or driver termination.
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received SIGINT, shutting down");
        }
        _ = &mut driver_task => {
            tracing::warn!("Network driver terminated, shutting down");
        }
        _ = status_handle => {
            tracing::debug!("Scheduler terminated")
        }
    }

    // NOTE: Graceful shutdown sequence:
    //
    // 1. Stop network interfaces and ensure the driver is completed before telemetry shutdown.
    drop(network);
    if !driver_task.is_finished() {
        driver_task.abort();
    }
    let _ = driver_task.await;
    if !tracker_task.is_finished() {
        tracker_task.abort();
    }
    let _ = tracker_task.await;
    // 2. Flush any remaining telemetry and shutdown providers, do not log anything after this point
    metrics.shutdown().into_diagnostic()?;
    tracing.shutdown().into_diagnostic()?;
    logging.shutdown().into_diagnostic()?;

    Ok(())
}

fn get_executor_with_dataset(
    config: &DiLoCo,
    peer_id: PeerId,
    dataset: String,
    parameter_server: PeerId,
) -> Executor {
    Executor::DiLoCoTransformer {
        model: config.model.clone().into(),
        data: Fetch::scheduler(peer_id, dataset),
        updates: Receive::peers(vec![parameter_server]),
        results: Send::peers(vec![parameter_server], SelectionStrategy::One),
        config: config.worker.clone().into(),
    }
}

// Find the data provider for the requested dataset
async fn get_data_provider(
    network: &Network,
    dataset: &str,
) -> miette::Result<(PeerId, DataRecord)> {
    let record = network.get(dataset).await.map_err(|e| {
        miette::miette!("No data provider found for dataset \"{}\": {}", dataset, e)
    })?;

    match record.publisher {
        Some(data_provider) => match serde_json::from_slice(&record.value) {
            Ok(dataset_record) => Ok((data_provider, dataset_record)),
            Err(e) => Err(miette::miette!(
                "Failed to parse dataset record for dataset \"{}\": {}",
                dataset,
                e
            )),
        },
        None => Err(miette::miette!(
            "No data provider found for dataset \"{}\"",
            dataset
        )),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Commands::Init { output } => {
            fs::write(output, &to_toml(&Config::default())?).into_diagnostic()?;

            println!("Configuration written to: {output:?}");
            Ok(())
        }
        args @ Commands::Probe {
            config_file,
            address,
            timeout,
            ..
        } => {
            let config = builder::<Config>()
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
            let config = builder::<Config>()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_"))
                .with_provider(Env::prefixed("OTEL_"))
                .with_provider(Serialized::defaults(args))
                .build()?
                .validate()?;

            return run(config).await;
        }
    }
}
