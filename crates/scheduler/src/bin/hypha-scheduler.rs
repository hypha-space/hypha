//! Scheduler binary.

use std::{fs, sync::Arc, time::Duration};

use clap::Parser;
use figment::{
    providers::{Env, Format, Serialized, Toml},
    value::Map,
};
use futures_util::{StreamExt, future::join_all};
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_messages::{
    AggregateExecutorConfig, AggregateExecutorDescriptor, DataRecord, Fetch, JobSpec,
    TrainExecutorConfig, TrainExecutorDescriptor, WorkerSpec, health,
};
use hypha_network::{
    cert::identity_from_private_key, dial::DialInterface,
    external_address::ExternalAddressInterface, kad::KademliaInterface, listen::ListenInterface,
    request_response::RequestResponseInterfaceExt, swarm::SwarmDriver,
};
use hypha_resources::{Resources, WeightedResourceEvaluator};
use hypha_scheduler::{
    allocator::GreedyWorkerAllocator,
    config::Config,
    metrics_bridge::{AimConnector, MetricsBridge, NoOpConnector},
    network::Network,
    pool::{Pool, PoolConfig, PoolWithStatistics},
    scheduler_config::Job as SchedulerJob,
    scheduling::{batch_scheduler::BatchScheduler, data_scheduler::DataScheduler},
    simulation::BasicSimulation,
    statistics::RunningMean,
    task::Task,
};
use hypha_telemetry as telemetry;
use libp2p::{Multiaddr, PeerId, multiaddr::Protocol};
use miette::{IntoDiagnostic, Result};
use serde_json::Value;
use tokio_retry::{
    Retry,
    strategy::{ExponentialBackoff, jitter},
};
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
    let gateway_peer_ids = Retry::spawn(
        ExponentialBackoff::from_millis(100).map(jitter).take(3),
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

    // Wait until DHT bootstrapping is done.
    network.wait_for_bootstrap().await.into_diagnostic()?;

    let token = CancellationToken::new();

    let SchedulerJob::Diloco(diloco_config) = &config.scheduler_config().job;

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

    let worker_pool = Pool::new(
        GreedyWorkerAllocator::new(network.clone(), WeightedResourceEvaluator::default()),
        PoolConfig {
            name: "workers".into(),
            spec: worker_spec.clone(),
            price: worker_price,
            min: diloco_config.resources.worker_pool.min as usize,
            target: diloco_config.resources.worker_pool.target as usize,
            grace: Duration::from_millis(diloco_config.resources.worker_pool.grace_ms),
        },
    );
    let worker_pool = PoolWithStatistics::<RunningMean>::new(worker_pool);
    let worker_handle = worker_pool.handle();

    let parameter_pool = Pool::new(
        GreedyWorkerAllocator::new(network.clone(), WeightedResourceEvaluator::default()),
        PoolConfig {
            name: "parameter-servers".into(),
            spec: parameter_server_spec.clone(),
            price: parameter_server_price,
            min: diloco_config.resources.parameter_server_pool.min as usize,
            target: diloco_config.resources.parameter_server_pool.target as usize,
            grace: Duration::from_millis(diloco_config.resources.parameter_server_pool.grace_ms),
        },
    );
    let parameter_handle = parameter_pool.handle();

    let dataset = diloco_config.dataset.dataset.clone();
    let (data_provider, dataset_record) = get_data_provider(&network, dataset.as_str()).await?;

    let data_scheduler = DataScheduler::new(
        network.clone(),
        data_provider,
        dataset.clone(),
        dataset_record.slice_hashes,
    );

    let tracker_task = tokio::spawn(
        data_scheduler
            .run(token.clone())
            .await
            .expect("network ready"),
    );

    let job_id = Uuid::new_v4();

    // Control the max allowed batch size.
    let max_batch_size = match diloco_config.rounds.max_batch_size {
        Some(bs) => bs as f64,
        _ => f64::MAX,
    };
    let base_gpu = worker_spec.resources.gpu();
    let max_batch_size_cfg = diloco_config.rounds.max_batch_size;
    let batch_sizer = Arc::new(move |resources: &Resources| {
        let raw = if base_gpu > 0.0 {
            (resources.gpu() / base_gpu).floor() as u32
        } else {
            0
        };
        match max_batch_size_cfg {
            Some(max) => raw.min(max),
            None => raw,
        }
    });

    let mut metrics_bridge = match config.status_bridge() {
        Some(value) => MetricsBridge::new(Box::new(AimConnector::new(value))),
        None => MetricsBridge::new(Box::new(NoOpConnector::new())),
    };

    // Spawn dispatcher for parameter servers.
    let parameter_dispatcher = {
        let network = network.clone();
        let diloco_config = diloco_config.clone();

        tokio::spawn(parameter_pool.for_each_concurrent(None, move |parameter_server| {
            let network = network.clone();
            let diloco_config = diloco_config.clone();

            async move {
                match parameter_server {
                    Ok(parameter_server) => {
                        tracing::debug!(%job_id, peer_id = %parameter_server.peer_id, "Parameter server started");

                        let job_spec = JobSpec {
                            job_id,
                            executor: AggregateExecutorDescriptor::new(
                                PARAMETER_SERVER_EXECUTOR_NAME,
                            )
                            .into_executor(AggregateExecutorConfig {
                                optimizer: diloco_config.outer_optimizer.clone(),
                            })
                            .into(),
                        };

                        match Task::try_new(network, job_spec, &[parameter_server.peer_id]).await {
                            Ok(task) => {
                                tracing::debug!(%job_id, peer_id = %parameter_server.peer_id,
                                    "Dispatched parameter server job");
                                tokio::spawn(task.for_each(|_| async {}));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    %job_id,
                                    peer_id = %parameter_server.peer_id,
                                    "Failed to dispatch parameter server job"
                                );
                            }
                        }
                    }
                    Err(e) => tracing::error!(error=?e, "Worker failed"),
                }
            }
        }))
    };

    // Spawn dispatcher for workers.
    let worker_dispatcher = {
        let network = network.clone();
        let diloco_config = diloco_config.clone();
        let worker_spec = worker_spec.clone();
        let dataset = dataset.clone();

        tokio::spawn(worker_pool.for_each_concurrent(None, move |worker| {
            let network = network.clone();
            let diloco_config = diloco_config.clone();
            let worker_spec = worker_spec.clone();
            let dataset = dataset.clone();

            async move {
                match worker {
                    Ok(worker) => {
                        tracing::debug!(%job_id, peer_id = %worker.peer_id, "Worker started");

                        let batch_size = (worker.resources.gpu() / worker_spec.resources.gpu())
                            .floor()
                            .min(max_batch_size) as u32;

                        let job_spec = JobSpec {
                            job_id,
                            executor: TrainExecutorDescriptor::new(TRAIN_EXECUTOR_NAME)
                                .into_executor(TrainExecutorConfig {
                                    model: diloco_config.model.clone().into(),
                                    data: Fetch::scheduler(peer_id, dataset.clone()),
                                    optimizer: diloco_config.inner_optimizer.clone(),
                                    batch_size,
                                    preprocessor: diloco_config
                                        .preprocessor
                                        .clone()
                                        .map(|p| p.into()),
                                    scheduler: None,
                                })
                                .into(),
                        };

                        match Task::try_new(network, job_spec, &[worker.peer_id]).await {
                            Ok(task) => {
                                tracing::debug!(%job_id, peer_id = %worker.peer_id,
                                    "Dispatched worker job");
                                tokio::spawn(task.for_each(|_| async {}));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    %job_id,
                                    peer_id = %worker.peer_id,
                                    "Failed to dispatch worker job"
                                );
                            }
                        }
                    }
                    Err(e) => tracing::error!(error=?e, "💥 Worker failed"),
                }
            }
        }))
    };

    let (metrics_rx, mut batch_scheduler_handle) = BatchScheduler::run::<RunningMean, BasicSimulation>(
        network.clone(),
        worker_handle.clone(),
        parameter_handle.clone(),
        job_id,
        diloco_config.resources.worker_pool.min as usize,
        Duration::from_millis(diloco_config.resources.worker_pool.grace_ms),
        diloco_config.rounds.avg_samples_between_updates,
        diloco_config.rounds.update_rounds,
        diloco_config.model_destination.clone(),
        batch_sizer.clone(),
        token.clone(),
    )
    .await
    .into_diagnostic()?;

    metrics_bridge.register_stream(ReceiverStream::new(metrics_rx));

    let cancel_token = token.clone();
    let mut status_handle = tokio::spawn(async move {
        if let Err(e) = metrics_bridge.run(cancel_token).await {
            tracing::error!(error = %e, "Status bridge failed");
        }
    });

    // Wait for Ctrl-C or driver termination.
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received SIGINT, shutting down");
        }
        _ = &mut driver_task => {
            tracing::warn!("Network driver terminated, shutting down");
        }
        _ = &mut status_handle => {
            tracing::error!("Status bridge terminated, shutting down");
        }
        _ = &mut batch_scheduler_handle => {
            tracing::info!("Batch scheduler finished, shutting down");
        }
    }

    // NOTE: Graceful shutdown sequence:
    //
    // 1. Stop components
    // 2. Stop network interfaces and ensure the driver is completed
    // 3. Flush any remaining telemetry and shutdown providers
    //
    // IMPORTANT: Do not log anything after shutting down the telemetry providers

    if !batch_scheduler_handle.is_finished() {
        batch_scheduler_handle.abort();
    }
    let _ = batch_scheduler_handle.await;

    if !parameter_dispatcher.is_finished() {
        parameter_dispatcher.abort();
    }
    let _ = parameter_dispatcher.await;

    if !worker_dispatcher.is_finished() {
        worker_dispatcher.abort();
    }
    let _ = worker_dispatcher.await;

    if !status_handle.is_finished() {
        status_handle.abort();
    }
    let _ = status_handle.await;

    if !tracker_task.is_finished() {
        tracker_task.abort();
    }
    let _ = tracker_task.await;
    drop(network);
    if !driver_task.is_finished() {
        driver_task.abort();
    }
    let _ = driver_task.await;

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
