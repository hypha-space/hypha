//! Gateway binary.

use std::{error::Error, fs, net::SocketAddr, path::PathBuf};

use clap::Parser;
use hypha_gateway::network::Network;
use hypha_network::{
    cert::{load_certs_from_pem, load_crls_from_pem, load_private_key_from_pem},
    listen::ListenInterface,
    swarm::SwarmDriver,
    utils::multiaddr_from_socketaddr,
};
use tokio::signal::unix::{SignalKind, signal};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "hypha-gateway",
    version,
    about = "Hypha Gateway Node",
    long_about = "Runs the Hypha Gateway facilitating network connectivity between peers.",
    after_help = "For more information, see the project documentation."
)]
struct Opt {
    #[clap(long)]
    cert_file: PathBuf,
    #[clap(long)]
    key_file: PathBuf,
    #[clap(long)]
    ca_cert_file: PathBuf,
    #[clap(long)]
    crl_file: Option<PathBuf>,
    #[clap(long, default_value = "[::1]:8888")]
    listen_address: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let opt = Opt::parse();

    // Load certificates and private key
    let cert_pem = fs::read(&opt.cert_file)?;
    let key_pem = fs::read(&opt.key_file)?;
    let ca_cert_pem = fs::read(&opt.ca_cert_file)?;

    let cert_chain = load_certs_from_pem(&cert_pem)?;
    let private_key = load_private_key_from_pem(&key_pem)?;
    let ca_certs = load_certs_from_pem(&ca_cert_pem)?;

    // Optionally load CRLs
    let crls = if let Some(crl_file) = opt.crl_file {
        let crl_pem = fs::read(&crl_file)?;
        load_crls_from_pem(&crl_pem)?
    } else {
        vec![]
    };

    let (network, network_driver) = Network::create(cert_chain, private_key, ca_certs, crls)?;
    let driver_future = tokio::spawn(network_driver.run());

    network
        .listen(multiaddr_from_socketaddr(opt.listen_address)?)
        .await?;
    tracing::info!("Successfully listening");

    let mut sigterm = signal(SignalKind::terminate())?;

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
