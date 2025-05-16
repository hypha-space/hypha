mod network;

use std::{error::Error, time::Duration};

use futures::{SinkExt, StreamExt};

use tokio::{join, task};
use tokio_util::{
    codec::{Framed, LengthDelimitedCodec},
    compat::{FuturesAsyncReadCompatExt, FuturesAsyncWriteCompatExt},
};

use clap::Parser;
use hypha_api::Response;
use hypha_network::{
    dial::DialInterface,
    gossipsub::GossipsubInterface,
    kad::KademliaInterface,
    listen::ListenInterface,
    request_response::RequestResponseInterface,
    stream::{StreamReceiverInterface, StreamSenderInterface},
    swarm::SwarmDriver,
    utils::generate_ed25519,
};
use libp2p::multiaddr::{Multiaddr, Protocol};
use tracing_subscriber::{EnvFilter, fmt::format};

use crate::network::Network;

#[derive(Debug, Parser)]
#[clap(name = "hypha")]
struct Opt {
    #[clap(long)]
    secret_key_seed: u8,
    #[clap(long, short, default_value = "0")]
    port: String,
    #[clap(long)]
    gateway_address: String,
    #[clap(long)]
    forward_peer: String,
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
        .listen(format!("/ip4/0.0.0.0/udp/{}/quic-v1", opt.port).parse()?)
        .await?;
    network
        .listen(format!("/ip4/0.0.0.0/tcp/{}", opt.port).parse()?)
        .await?;

    tracing::info!("Successfully listening");

    // Dial the gatewaty address
    let gateway_peer_id = network.dial(gateway_address.clone()).await?;

    tracing::info!(gateway_address=%gateway_peer_id, "Dialed Gateway");

    // Wait a bit until DHT bootstrapping is done.
    // Once we receive an 'Identify' message, bootstrapping will start.
    // TODO: Provide a way to wait for this event
    tokio::time::sleep(Duration::from_secs(2)).await;

    network
        .listen(
            gateway_address
                .with_p2p(gateway_peer_id)
                .unwrap()
                .with(Protocol::P2pCircuit),
        )
        .await?;

    network
        .store(libp2p::kad::Record {
            key: libp2p::kad::RecordKey::new(&"cpu"),
            value: "test".as_bytes().to_vec(),
            publisher: None,
            expires: None,
        })
        .await?;
    tracing::info!("Stored 'cpu' record");

    let mut streams = network.streams()?;

    let forward_peer = opt.forward_peer.parse()?;

    while let Some((peer, stream_in)) = streams.next().await {
        tracing::info!(peer = %peer, "New incoming stream");
        let network_clone = network.clone();
        task::spawn(async move {
            let stream_out = match network_clone.stream(forward_peer).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("Failed to create stream to forward peer: {:?}", e);
                    return;
                }
            };

            let mut message_in = Framed::new(stream_in.compat(), LengthDelimitedCodec::new());
            let mut message_out = Framed::new(stream_out.compat(), LengthDelimitedCodec::new());
            let mut count = 0;
            while let Some(frame_result) = message_in.next().await {
                match frame_result {
                    Ok(bytes) => {
                        count += 1;
                        tracing::info!(peer = %peer, count, "Received frame, forwarding");
                        // Do something with the bytes...
                        let _ = message_out.send(bytes.freeze()).await;
                    }
                    Err(e) => {
                        tracing::error!("Framing error: {:?}", e);
                        break;
                    }
                }
            }
        });
    }

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
