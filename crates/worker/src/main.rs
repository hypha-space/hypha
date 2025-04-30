mod network;

use std::{error::Error, time::Duration};

use clap::Parser;
use hypha_network::{
    dial::DialInterface,
    gossipsub::{GossipsubEvent, GossipsubInterface},
    kad::KademliaInterface,
    listen::ListenInterface,
    request_response::RequestResponseInterface,
    swarm::SwarmDriver,
    utils::generate_ed25519,
};
use libp2p::{Multiaddr, PeerId};
use network::Event;
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

    let (network, network_driver, mut event_receiver) = Network::create(local_key)?;
    tokio::spawn(network_driver.run());

    network
        .listen("/ip4/0.0.0.0/udp/0/quic-v1".parse()?)
        .await?;
    network.listen("/ip4/0.0.0.0/tcp/0".parse()?).await?;
    tracing::info!("Successfully listening");

    // Dial the gatewaty address
    let _gateway_peer_id = network.dial(gateway_address).await?;

    // Wait a bit until DHT bootstrapping is done.
    // Once we receive an 'Identify' message, bootstrapping will start.
    // TODO: Provide a way to wait for this event
    tokio::time::sleep(Duration::from_secs(2)).await;

    network
        .store(libp2p::kad::Record {
            key: libp2p::kad::RecordKey::new(&"cpu"),
            value: "test".as_bytes().to_vec(),
            publisher: None,
            expires: None,
        })
        .await?;
    tracing::info!("Stored 'cpu' record");

    network.subscribe("messages").await?;

    while let Some(event) = event_receiver.recv().await {
        match event {
            Event::Gossipsub(event) => match event {
                GossipsubEvent::Message(_, data) => {
                    tracing::info!("Received message: {:?}", data);

                    let peer_id = PeerId::from_bytes(data.as_slice()).unwrap();
                    tracing::info!("Sending request to peer {}", peer_id);
                    let response = network.request(peer_id, hypha_api::Request::Work()).await;
                    tracing::info!("Received response from peer {}: {:?}", peer_id, response);
                }
            },
            _ => tracing::warn!("Unexpected event received"),
        }
    }

    Ok(())
}
