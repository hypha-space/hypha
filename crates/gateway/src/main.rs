mod network;

use std::error::Error;

use clap::Parser;
use hypha_network::{listen::ListenInterface, swarm::SwarmDriver, utils::generate_ed25519};
use tracing_subscriber::EnvFilter;

use crate::network::Network;

#[derive(Debug, Parser)]
#[clap(name = "hypha")]
struct Opt {
    #[clap(long)]
    secret_key_seed: u8,
    #[clap(short)]
    port: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let opt = Opt::parse();

    tracing::info!(
        "Starting worker with secret key seed {}",
        opt.secret_key_seed
    );

    let local_key = generate_ed25519(opt.secret_key_seed).expect("only errors on wrong length");

    let (network, network_driver) = Network::create(local_key)?;
    let task = tokio::spawn(network_driver.run());

    network
        .listen(format!("/ip4/0.0.0.0/udp/{}/quic-v1", opt.port).parse()?)
        .await?;
    network
        .listen(format!("/ip4/0.0.0.0/tcp/{}", opt.port).parse()?)
        .await?;
    tracing::info!("Successfully listening");

    let _ = task.await?;
    Ok(())
}
