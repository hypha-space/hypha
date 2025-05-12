mod network;

use std::{error::Error, time::Duration};

use clap::Parser;
use hypha_api::Request;
use hypha_network::{
    dial::DialInterface, gossipsub::GossipsubInterface, kad::KademliaInterface,
    listen::ListenInterface, request_response::RequestResponseInterface, swarm::SwarmDriver,
    utils::generate_ed25519,
};
use libp2p::Multiaddr;
use tracing_subscriber::EnvFilter;

use crate::network::Network;

#[derive(Debug, Parser)]
#[clap(name = "hypha")]
struct Opt {
    #[clap(long)]
    secret_key_seed: u8,
    #[clap(long)]
    gateway_address: String,
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
    let gateway_address = opt.gateway_address.parse::<Multiaddr>()?;

    let (network, network_driver) = Network::create(local_key.clone())?;
    tokio::spawn(network_driver.run());

    network
        .listen("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)
        .await?;
    network.listen("/ip4/0.0.0.0/tcp/0".parse()?).await?;
    tracing::info!("Successfully listening");

    // Dial the gateway address
    let _gateway_peer_id = network.dial(gateway_address).await?;

    // Wait a bit until DHT bootstrapping is done.
    // Once we receive an 'Identify' message, bootstrapping will start.
    // TODO: Provide a way to wait for this event
    tokio::time::sleep(Duration::from_secs(2)).await;

    let record = network.get("cpu").await?;
    tracing::info!(record=?record,"Found CPU record");

    let k = local_key.public().to_peer_id().to_base58();
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        tracing::info!("Tick");
        let _ = network.publish("messages", k.clone()).await;

        let res = network
            .request(
                "12D3KooWH3uVF6wv47WnArKHk5p6cvgCJEb74UTmxztmQDc298L3".parse()?,
                Request::Work(),
            )
            .await;

        tracing::info!(response=?res,"Got a response");
    }
}
