//! Scheduler binary.

use std::{fs, sync::Arc, time::Duration};

use clap::Parser;
use figment::{
    providers::{Env, Format, Serialized, Toml},
    value::Map,
};
use futures_util::future::{join_all, select_all};
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_messages::{
    AggregateExecutorConfig, AggregateExecutorDescriptor, DataRecord, Fetch, JobSpec, Receive,
    SelectionStrategy, Send, TrainExecutorConfig, TrainExecutorDescriptor, WorkerSpec, health,
};
use hypha_network::{
    cert::identity_from_private_key, dial::DialInterface,
    external_address::ExternalAddressInterface, kad::KademliaInterface, listen::ListenInterface,
    request_response::RequestResponseInterfaceExt, swarm::SwarmDriver,
};
use hypha_resources::WeightedResourceEvaluator;
use hypha_scheduler::{
    allocator::{Allocator, GreedyWorkerAllocator},
    config::Config,
    metrics_bridge::{AimConnector, MetricsBridge, NoOpConnector},
    network::Network,
    scheduler_config::Job as SchedulerJob,
    scheduling::{batch_scheduler::BatchScheduler, data_scheduler::DataScheduler},
    simulation::BasicSimulation,
    statistics::RunningMean,
    task::Task,
    tracker::progress::ProgressTracker,
};
use hypha_telemetry as telemetry;
use libp2p::{Multiaddr, PeerId, multiaddr::Protocol};
use miette::{IntoDiagnostic, Result};
use serde_json::Value;
use tokio::{sync::Mutex, time::sleep};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    EnvFilter, Layer, Registry, layer::SubscriberExt, util::SubscriberInitExt,
};
use uuid::Uuid;

const TRAIN_EXECUTOR_NAME: &str = "diloco-transformer";
const PARAMETER_SERVER_EXECUTOR_NAME: &str = "parameter-server";

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

    let SchedulerJob::Diloco(diloco_config) = &config.scheduler_config().job;

    // NOTE: Create allocator for resource allocation alongside existing job management
    // TODO: Create `WeightedResourceEvaluator` for config so that the weights can be adjusted based on the job requirements.
    let allocator =
        GreedyWorkerAllocator::new(network.clone(), WeightedResourceEvaluator::default());

    tracing::info!("Starting worker allocation and job creation process");

    let worker_spec = WorkerSpec {
        resources: diloco_config.resources.worker,
        executor: vec![TrainExecutorDescriptor::new(TRAIN_EXECUTOR_NAME).into()],
    };

    let parameter_server_spec = WorkerSpec {
        resources: diloco_config.resources.parameter_server,
        executor: vec![AggregateExecutorDescriptor::new(PARAMETER_SERVER_EXECUTOR_NAME).into()],
    };

    let worker_price = diloco_config.resources.worker_price;
    let parameter_server_price = diloco_config.resources.parameter_server_price;

    // NOTE: Phase 1 - Allocate workers using the new allocator/arbiter protocol
    // First, request worker nodes for job execution
    let allocated_workers = match allocator
        .request(
            worker_spec.clone(),
            worker_price,
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

    // NOTE: We need to wait to allow workers to release tmp reservations before requesting parameter servers.
    // This will be fixed once we have introduced a proper worker pool with retry allocations.
    sleep(Duration::from_millis(1000)).await;

    // Request multiple workers to increase chances of two distinct peers
    let allocated_parameter_servers = match allocator
        .request(
            parameter_server_spec.clone(),
            parameter_server_price,
            None,
            1,
        )
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

    let data_scheduler = DataScheduler::new(
        network.clone(),
        data_provider,
        dataset.clone(),
        dataset_record.num_slices,
    );

    let tracker_task = tokio::spawn(
        data_scheduler
            .run(token.clone())
            .await
            .expect("network ready"),
    );

    let status_handle = if allocated_workers.len() >= diloco_config.resources.num_workers as usize
        && allocated_parameter_servers.len() == 1
    {
        let job_id = Uuid::new_v4();

        // Control the max allowed batch size.
        let max_batch_size = match diloco_config.rounds.max_batch_size {
            Some(bs) => bs as f64,
            _ => f64::MAX,
        };

        let worker_ids = allocated_workers
            .iter()
            .map(|w| w.peer_id())
            .collect::<Vec<_>>();

        let run_tracker = Arc::new(Mutex::new(ProgressTracker::<RunningMean>::new(
            allocated_parameter_servers[0].peer_id(),
            diloco_config.rounds.avg_samples_between_updates,
            diloco_config.rounds.update_rounds,
        )));

        let (metrics_rx, batch_scheduler_handle) = BatchScheduler::run::<
            RunningMean,
            BasicSimulation,
        >(
            network.clone(), run_tracker.clone(), job_id
        )
        .await
        .into_diagnostic()?;

        let parameter_server = &allocated_parameter_servers[0];

        let mut worker_tasks: Vec<Task> = Vec::with_capacity(allocated_workers.len());
        for w in allocated_workers.iter() {
            let batch_size = (w.resources().gpu() / worker_spec.resources.gpu())
                .floor()
                .min(max_batch_size) as u32;
            run_tracker
                .lock()
                .await
                .worker_tracker
                .add_worker(w.peer_id(), batch_size);
            let worker_task = Task::try_new(
                network.clone(),
                JobSpec {
                    job_id,
                    executor: TrainExecutorDescriptor::new(TRAIN_EXECUTOR_NAME)
                        .into_executor(TrainExecutorConfig {
                            model: diloco_config.model.clone().into(),
                            data: Fetch::scheduler(peer_id, dataset.to_string()),
                            updates: Send::peers(
                                vec![parameter_server.peer_id()],
                                SelectionStrategy::All,
                            ),
                            results: Receive::peers(vec![parameter_server.peer_id()]),
                            optimizer: diloco_config.inner_optimizer.clone(),
                            batch_size,
                            preprocessor: diloco_config.preprocessor.clone().map(|p| p.into()),
                            scheduler: None,
                        })
                        .into(),
                },
                &[w],
            )
            .await
            .into_diagnostic()?;
            worker_tasks.push(worker_task);
        }

        let _parameter_server_task = Task::try_new(
            network.clone(),
            JobSpec {
                job_id,
                executor: AggregateExecutorDescriptor::new(PARAMETER_SERVER_EXECUTOR_NAME)
                    .into_executor(AggregateExecutorConfig {
                        updates: Receive::peers(worker_ids.to_vec()),
                        results: Send::peers(worker_ids.to_vec(), SelectionStrategy::All),
                        optimizer: diloco_config.outer_optimizer.clone(),
                    })
                    .into(),
            },
            &[parameter_server],
        )
        .await
        .into_diagnostic()?;

        let mut metrics_bridge = match config.status_bridge() {
            Some(value) => MetricsBridge::new(Box::new(AimConnector::new(value))),
            None => MetricsBridge::new(Box::new(NoOpConnector::new())),
        };

        metrics_bridge.register_stream(ReceiverStream::new(metrics_rx));

        let cancel_token = token.clone();
        tokio::spawn(async move {
            tokio::select! {
                Err(e) = metrics_bridge.run(cancel_token) => {
                    tracing::error!(error = %e, "Status bridge failed");
                }
                _ = batch_scheduler_handle => {
                    tracing::info!("Batch Scheduler finished");
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
        (Err(e), index, _) = select_all(allocated_workers) => {
            tracing::error!(error = %e, "Worker {index} failed");
        }
        (Err(e), _, _) = select_all(allocated_parameter_servers) => {
            tracing::error!(error = %e, "Parameter server failed");
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
        args @ Commands::Init {
            output, name, job, ..
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
            if let Some(job_json) = job {
                let job: Value = serde_json::from_str(job_json).into_diagnostic()?;

                config_builder =
                    config_builder.with_provider(Serialized::default("scheduler", job));
            }

            let config = config_builder
                .with_provider(Serialized::defaults(&args))
                .build()?
                .validate()?;

            fs::write(output, &to_toml(&config.config)?).into_diagnostic()?;

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
