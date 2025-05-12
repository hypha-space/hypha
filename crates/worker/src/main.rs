mod network;

use std::{error::Error, fs, path::PathBuf, time::Duration};

use clap::Parser;
use hypha_network::{
    cert::{load_certs_from_pem, load_crls_from_pem, load_private_key_from_pem},
    dial::DialInterface,
    listen::ListenInterface,
    swarm::SwarmDriver,
};
use libp2p::Multiaddr;
use tracing_subscriber::EnvFilter;

use crate::network::Network;

#[derive(Debug, Parser)]
#[clap(name = "hypha")]
struct Opt {
    #[clap(long)]
    cert_file: PathBuf,
    #[clap(long)]
    key_file: PathBuf,
    #[clap(long)]
    ca_cert_file: PathBuf,
    #[clap(long)]
    crl_file: Option<PathBuf>,
    #[clap(long)]
    gateway_address: String,
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

    let gateway_address = opt.gateway_address.parse::<Multiaddr>()?;

    let (network, network_driver) = Network::create(cert_chain, private_key, ca_certs, crls)?;
    tokio::spawn(network_driver.run());

    network.listen("/ip4/0.0.0.0/tcp/0".parse()?).await?;
    tracing::info!("Successfully listening");

    // Dial the gateway address
    let _gateway_peer_id = network.dial(gateway_address).await?;

    tracing::info!(gateway_id = %_gateway_peer_id, "Connected to gateway");
    // Wait a bit until DHT bootstrapping is done.
    // Once we receive an 'Identify' message, bootstrapping will start.
    // TODO: Provide a way to wait for this event
    tokio::time::sleep(Duration::from_secs(2)).await;

    Ok(())
}
