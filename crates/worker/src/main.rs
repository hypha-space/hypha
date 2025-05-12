mod network;

use std::{error::Error, time::Duration};

use tokio::join;

use clap::Parser;
use futures_util::StreamExt;
use hypha_api::Response;
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

    let (network, network_driver) = Network::create(local_key)?;
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

    let requests = network
        .requests()
        .await?
        .for_each_concurrent(None, async |req| {
            tracing::info!("Got a request");
            match req {
                Ok(req) => {
                    let _ = network
                        .store(libp2p::kad::Record {
                            key: libp2p::kad::RecordKey::new(&"cpu"),
                            value: req.request_id.to_string().as_bytes().to_owned(),
                            publisher: None,
                            expires: None,
                        })
                        .await;
                    // TODO: We may want to retry sending a response
                    let _ = network
                        .respond(req.request_id, req.channel, Response::WorkDone())
                        .await;
                }
                Err(err) => {
                    tracing::error!("Error processing request: {:?}", err);
                }
            }
        });

    let messages = network
        .subscribe("messages")
        .await?
        .for_each_concurrent(None, async |m| {
            tracing::info!("Received message: {:?}", m);
        });

    join!(requests, messages);

    Ok(())
}
