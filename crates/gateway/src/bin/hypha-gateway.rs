use std::{
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use clap::Parser;
use figment::{
    providers::{Env, Format, Serialized, Toml},
    value::Map,
};
use futures_util::future::join_all;
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_gateway::{config::Config, network::Network};
use hypha_messages::health;
use hypha_network::{
    dial::DialInterface, external_address::ExternalAddressInterface, listen::ListenInterface,
    request_response::RequestResponseInterfaceExt, swarm::SwarmDriver,
};
use hypha_telemetry as telemetry;
use libp2p::Multiaddr;
use miette::{IntoDiagnostic, Result};
use tokio::signal::unix::{SignalKind, signal};
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
        Duration::from_secs(1),
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

    // NOTE: Ready when listening on all addresses.
    let ready = Arc::new(AtomicBool::new(false));

    let cert_chain = config.load_cert_chain()?;
    let private_key = config.load_key()?;
    let ca_certs = config.load_trust_chain()?;
    let crls = config.load_crls()?;

    let exclude_cidrs = config.exclude_cidr().clone();
    let (network, network_driver) =
        Network::create(cert_chain, private_key, ca_certs, crls, exclude_cidrs)
            .into_diagnostic()?;
    let mut driver_task = tokio::spawn(network_driver.run());

    // Register health handler responding with readiness
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

    // NOTE: This will not complete until we're listening on all addresses.
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

    // NOTE: Mark gateway as healthy once all listens are active (per requirement).
    ready.store(true, Ordering::Relaxed);

    // NOTE: This will not complete until all external addresses are added.
    join_all(
        config
            .external_addresses()
            .iter()
            .map(|address| network.add_external_address(address.clone()))
            .collect::<Vec<_>>(),
    )
    .await;

    // NOTE: Dial each gateway address (if any provided). The gateway attempts to connect
    // to all addresses and succeeds if any are reachable. Empty gateway_addresses is valid.
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
                            tracing::info!(address=%address, peer_id=%peer_id, "Connected to gateway");
                            Ok(peer_id)
                        }
                        Err(e) => {
                            tracing::warn!(address=%address, error=%e, "Failed to connect to gateway");
                            Err(e)
                        }
                    }
                }
            })
            .collect::<Vec<_>>(),
    )
    .await;

    let mut sigterm = signal(SignalKind::terminate()).into_diagnostic()?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received SIGINT, shutting down");
        }
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM, shutting down");
        }
        _ = health_handle => {
            tracing::info!("Health handler terminated, shutting down");
        }
        _ = &mut driver_task => {
            tracing::warn!("Network driver terminated, shutting down");
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
    // 2. Flush any remaining telemetry and shutdown providers, do not log anything after this point
    metrics.shutdown().into_diagnostic()?;
    tracing.shutdown().into_diagnostic()?;
    logging.shutdown().into_diagnostic()?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
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
