use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use clap::{Parser, Subcommand};
use figment::providers::{Env, Format, Serialized, Toml};
use futures_util::future::join_all;
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, cli, to_toml};
use hypha_gateway::{config::Config, network::Network};
use hypha_messages::health;
use hypha_network::{
    IpNet, dial::DialInterface, external_address::ExternalAddressInterface,
    listen::ListenInterface, request_response::RequestResponseInterfaceExt, swarm::SwarmDriver,
};
use hypha_telemetry as telemetry;
use libp2p::Multiaddr;
use miette::{IntoDiagnostic, Result};
use serde::Serialize;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::{
    EnvFilter, Layer, Registry, layer::SubscriberExt, util::SubscriberInitExt,
};

#[derive(Debug, Parser, Serialize)]
#[command(
    name = "hypha-gateway",
    version,
    about = "Hypha Gateway Node",
    long_about = "Runs the Hypha Gateway facilitating network connectivity between peers.",
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
        #[clap(short, long)]
        output: Option<PathBuf>,

        /// Optional name for this gateway node, defaults to "gateway"
        #[clap(short = 'n', long = "name", default_value = "gateway")]
        name: String,

        /// Set configuration values (can be used multiple times)
        /// Example: --set listen_addresses.0="/ip4/127.0.0.1/tcp/8080" --set external_addresses.0="/ip4/127.0.0.1/tcp/8080"
        #[clap(long = "set", value_parser = cli::parse_key_val)]
        #[serde(skip)]
        config_overrides: Vec<(String, String)>,
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

        /// Set configuration values (can be used multiple times)
        /// Example: --set listen_addresses.0="/ip4/127.0.0.1/tcp/8080"
        #[clap(long = "set", value_parser = cli::parse_key_val)]
        #[serde(skip)]
        config_overrides: Vec<(String, String)>,
    },
    #[serde(untagged)]
    Run {
        /// Path to the configuration file.
        #[clap(short, long("config"), default_value = "config.toml")]
        #[serde(skip)]
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

        /// Addresses to listen on (can be specified multiple times).
        #[clap(long("listen"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        listen_addresses: Option<Vec<Multiaddr>>,

        /// External addresses to advertise (can be specified multiple times).
        #[clap(long("external"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        external_addresses: Option<Vec<Multiaddr>>,

        /// CIDR exclusion (repeatable). Overrides config if provided.
        /// Example: --exclude-cidr 10.0.0.0/8 --exclude-cidr fc00::/7
        #[clap(long("exclude-cidr"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        exclude_cidr: Option<Vec<IpNet>>,

        /// Set configuration values (can be used multiple times)
        /// Example: --set listen_addresses.0="/ip4/127.0.0.1/tcp/8080"
        #[clap(long = "set", value_parser = cli::parse_key_val)]
        #[serde(skip)]
        config_overrides: Vec<(String, String)>,
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
        Duration::from_secs(1),
    )
    .into_diagnostic()?;

    telemetry::metrics::global::set_provider(metrics.provider());

    Registry::default()
        .with(tracing_subscriber::fmt::layer().with_filter(EnvFilter::from_default_env()))
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
        Commands::Init {
            output,
            name,
            config_overrides,
        } => {
            // Create config with defaults, allowing override via --set
            let mut config = Config::with_name(name);

            // Apply config overrides if provided
            if !config_overrides.is_empty() {
                let overrides_toml = cli::create_overrides_toml(config_overrides);
                let base_config = to_toml(&config).into_diagnostic()?;

                // Create a merged configuration by applying overrides
                let merged_figment = builder::<Config>()
                    .with_provider(Toml::string(&base_config))
                    .with_provider(Toml::string(&overrides_toml))
                    .build()?;

                config = merged_figment.config;
            }

            let output = output
                .clone()
                .unwrap_or_else(|| PathBuf::from(format!("{}-config.toml", name)));

            fs::write(&output, &to_toml(&config).into_diagnostic()?).into_diagnostic()?;

            println!("Configuration written to: {output:?}");
            Ok(())
        }
        args @ Commands::Probe {
            config_file,
            address,
            timeout,
            config_overrides,
            ..
        } => {
            let mut config_builder = builder::<Config>()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_"))
                .with_provider(Serialized::defaults(&args));

            config_builder = cli::apply_config_overrides(config_builder, config_overrides)?;
            let config = config_builder.build()?;

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
        args @ Commands::Run {
            config_file,
            config_overrides,
            ..
        } => {
            let mut config_builder = builder::<Config>()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_"))
                .with_provider(Env::prefixed("OTEL_"))
                .with_provider(Serialized::defaults(args));

            config_builder = cli::apply_config_overrides(config_builder, config_overrides)?;
            let config = config_builder.build()?;

            return run(config).await;
        }
    }
}
