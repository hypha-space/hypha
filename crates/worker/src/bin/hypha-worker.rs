//! Worker binary.

use std::{
    env, fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use clap::Parser;
use figment::{
    providers::{Env, Format, Serialized, Toml},
    value::Map,
};
use futures_util::future::join_all;
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_messages::health;
use hypha_network::{
    dial::DialInterface, external_address::ExternalAddressInterface, kad::KademliaInterface,
    listen::ListenInterface, request_response::RequestResponseInterfaceExt, swarm::SwarmDriver,
};
use hypha_telemetry as telemetry;
use hypha_worker::{
    arbiter::Arbiter, config::Config, connector::Connector, job_manager::JobManager,
    lease_manager::ResourceLeaseManager, network::Network,
    request_evaluator::WeightedResourceRequestEvaluator, resources::StaticResourceManager,
};
use libp2p::{Multiaddr, multiaddr::Protocol};
use miette::{IntoDiagnostic, Result};
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
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

    // NOTE: Healthy (readiness) is defined as:
    // - listening on all addresses
    // - DHT bootstrap complete
    let ready = Arc::new(AtomicBool::new(false));

    // Load certificates and private key
    let exclude_cidrs = config.exclude_cidr().clone();

    let (network, network_driver) = Network::create(
        config.load_cert_chain()?,
        config.load_key()?,
        config.load_trust_chain()?,
        config.load_crls()?,
        exclude_cidrs,
    )
    .into_diagnostic()?;

    // NOTE: Spawn network driver and keep handle for coordinated shutdown.
    let mut driver_task = tokio::spawn(network_driver.run());

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

    // Dial each gateway and, optionally, set up a relay circuit listen via it.
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

    // NOTE: Wait until DHT bootstrapping is done.
    network.wait_for_bootstrap().await.into_diagnostic()?;

    // NOTE: Mark worker as ready only after listening + bootstrap complete.
    ready.store(true, Ordering::Relaxed);

    let token = CancellationToken::new();
    let executors = config.executors().to_vec();

    // NOTE: Create arbiter for resource allocation - this is the primary worker allocation mechanism
    let work_dir_base = if config.work_dir().is_absolute() {
        config.work_dir().clone()
    } else {
        // NOTE: Align worker jobs with driver expectations by normalizing relative paths once.
        env::current_dir()
            .into_diagnostic()?
            .join(config.work_dir())
    };

    let arbiter = Arbiter::new(
        ResourceLeaseManager::new(StaticResourceManager::new(
            config.resources(),
            executors
                .iter()
                .map(|cfg| cfg.descriptor())
                .collect::<Vec<_>>(),
        )),
        WeightedResourceRequestEvaluator::default(),
        network.clone(),
        JobManager::new(
            Connector::new(network.clone()),
            network.clone(),
            work_dir_base,
            executors,
        ),
    );

    let arbiter_handle = tokio::spawn({
        let token = token.clone();
        async move {
            if let Err(e) = arbiter.run(token).await {
                tracing::error!(error = %e, "Arbiter failed");
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
        _ = &mut driver_task => {
            tracing::warn!("Network driver terminated, shutting down");
        }
        _ = health_handle => {
            tracing::info!("Health handler terminated, shutting down");
        }

    }

    // NOTE: Graceful shutdown sequence:
    //
    // 1. Signal the shutdown and wait for the arbiter to complete.
    token.cancel();
    let _ = arbiter_handle.await;
    // 2. Stop network interfaces and ensure the driver is completed before telemetry shutdown.
    drop(network);
    if !driver_task.is_finished() {
        driver_task.abort();
    }
    let _ = driver_task.await;
    // 3. Flush any remaining telemetry and shutdown providers, do not log anything after this point
    metrics.shutdown().into_diagnostic()?;
    tracing.shutdown().into_diagnostic()?;
    logging.shutdown().into_diagnostic()?;

    Ok(())
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        args @ Commands::Init { output, name, .. } => {
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

            let config = config_builder
                .with_provider(Serialized::defaults(&args))
                .build()?
                .validate()?;

            fs::write(output, &to_toml(&config.config).into_diagnostic()?).into_diagnostic()?;

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
                .with_provider(Serialized::defaults(&args))
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
            let d = std::time::Duration::from_millis(*timeout);
            tokio::time::timeout(d, async move {
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

            run(config).await
        }
    }
}
