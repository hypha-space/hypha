use std::{error::Error, fs, path::PathBuf};

use clap::{Parser, Subcommand};
use figment::providers::{Env, Format, Serialized, Toml};
use hypha_config::LayeredConfig;
use hypha_gateway::{config::Config, network::Network};
use hypha_network::{
    listen::ListenInterface,
    swarm::SwarmDriver,
    utils::{multiaddr_from_socketaddr, multiaddr_from_socketaddr_quic},
};
use miette::{IntoDiagnostic, Result};
use serde::Serialize;
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

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
        #[clap(short, long, default_value = "config.toml")]
        output: PathBuf,
    },
    #[serde(untagged)]
    Run {
        /// Path to the configuration file.
        #[clap(short, long("config"), default_value = "config.toml")]
        #[serde(skip)]
        config_file: PathBuf,

        /// Address to listen on.
        #[clap(long("listen"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        listen_address: Option<std::net::SocketAddr>,
    },
}

async fn run(config: Config) -> Result<(), Box<dyn Error>> {
    let cert_chain = config.load_cert_chain()?;
    let private_key = config.load_key()?;
    let ca_certs = config.load_trust_chain()?;
    let crls = config.load_crls()?;

    let (network, network_driver) =
        Network::create(cert_chain, private_key, ca_certs, crls).into_diagnostic()?;
    let driver_future = tokio::spawn(network_driver.run());

    network
        .listen(multiaddr_from_socketaddr(config.listen_address()).into_diagnostic()?)
        .await?;
    network
        .listen(multiaddr_from_socketaddr_quic(config.listen_address()).into_diagnostic()?)
        .await?;

    tracing::info!("Successfully listening");

    let mut sigterm = signal(SignalKind::terminate()).into_diagnostic()?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received SIGINT, shutting down");
        }
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM, shutting down");
        }
        _ = driver_future => {
            tracing::warn!("Network driver terminated, shutting down");
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match &cli.command {
        Commands::Init { output } => {
            fs::write(output, &Config::default().to_toml().into_diagnostic()?).into_diagnostic()?;

            println!("Configuration written to: {output:?}");
            Ok(())
        }
        args @ Commands::Run { config_file, .. } => {
            let config = Config::builder()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_GATEWAY_"))
                .with_provider(Serialized::defaults(args))
                .build()
                .into_diagnostic()?;

            return run(config).await;
        }
    }
}
