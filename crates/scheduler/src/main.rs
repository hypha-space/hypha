mod network;

use std::{
    error::Error,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bytes::Bytes;
use ciborium;
use futures::{SinkExt, StreamExt};
use rand::{RngCore, thread_rng};
use serde::{Deserialize, Serialize};

use tokio::task;
use tokio_util::{
    codec::{Framed, LengthDelimitedCodec},
    compat::FuturesAsyncReadCompatExt,
};

use clap::Parser;

use hypha_network::{
    dial::DialInterface,
    kad::KademliaInterface,
    listen::ListenInterface,
    stream::{StreamReceiverInterface, StreamSenderInterface},
    swarm::SwarmDriver,
    utils::generate_ed25519,
};
use libp2p::multiaddr::{Multiaddr, Protocol};
use tracing_subscriber::EnvFilter;

use crate::network::Network;

#[derive(Debug, Parser)]
#[clap(name = "hypha")]
struct Opt {
    #[clap(long)]
    secret_key_seed: u8,
    #[clap(long)]
    gateway_address: String,
    #[clap(long)]
    forward_peer: String,
}

#[derive(Serialize, Deserialize)]
struct TimestampedMessage {
    timestamp: u64,
    data: Vec<u8>, // Random data payload
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
    let gateway_peer_id = network.dial(gateway_address.clone()).await?;

    network
        .listen(
            gateway_address
                .with_p2p(gateway_peer_id)
                .unwrap()
                .with(Protocol::P2pCircuit),
        )
        .await?;
    // Wait a bit until DHT bootstrapping is done.
    // Once we receive an 'Identify' message, bootstrapping will start.
    // TODO: Provide a way to wait for this event
    tokio::time::sleep(Duration::from_secs(2)).await;

    let record = network.get("cpu").await?;
    tracing::info!(record=?record,"Found CPU record");

    let network_clone = network.clone();
    task::spawn(async move {
        let mut streams = match network_clone.streams() {
            Ok(s) => s,
            Err(error) => {
                tracing::error!(error=?error, "Failed to accept streams");
                return;
            }
        };

        while let Some((peer, stream_in)) = streams.next().await {
            let mut message_in = Framed::new(stream_in.compat(), LengthDelimitedCodec::new());

            while let Some(frame_result) = message_in.next().await {
                match frame_result {
                    Ok(bytes) => {
                        // Get current time for latency calculation
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as u64;

                        // Deserialize the message to get the timestamp
                        match ciborium::de::from_reader::<TimestampedMessage, _>(&bytes[..]) {
                            Ok(msg) => {
                                let latency = now - msg.timestamp;
                                tracing::info!(
                                    peer = %peer,
                                    latency_ms = %latency,
                                    message_size = %msg.data.len(),
                                    "Received message from peer {}: latency = {} ms, size = {} bytes",
                                    peer,
                                    latency,
                                    msg.data.len()
                                );
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Failed to deserialize message from {}: {:?}",
                                    peer,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Framing error: {:?}", e);
                        break;
                    }
                }
            }
        }
    });

    let forward_peer = opt.forward_peer.parse()?;
    let mut rng = thread_rng();

    loop {
        match network.stream(forward_peer).await {
            Ok(stream_out) => {
                let mut message_out = Framed::new(stream_out.compat(), LengthDelimitedCodec::new());

                loop {
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    let mut buf = vec![0u8; 40 * 1024];
                    rng.fill_bytes(&mut buf);

                    // Get current timestamp in milliseconds
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;

                    // Create and serialize the timestamped message
                    let msg = TimestampedMessage {
                        timestamp,
                        data: buf,
                    };

                    let mut serialized = Vec::new();
                    ciborium::ser::into_writer(&msg, &mut serialized).unwrap();
                    tracing::info!("Sennding {}", serialized.len());
                    match message_out.send(Bytes::from(serialized)).await {
                        Ok(_) => {
                            tracing::debug!("Sent message with timestamp {}", timestamp);
                        }
                        Err(e) => {
                            tracing::error!("Failed to send message: {:?}. Reconnecting...", e);
                            break; // Break inner loop to reconnect
                        }
                    };
                }
            }
            Err(e) => {
                tracing::error!(
                    "Failed to create stream to forward peer {}: {:?}",
                    forward_peer,
                    e
                );
            }
        };
    }

    // loop {
    //     tokio::time::sleep(Duration::from_millis(500)).await;
    //     tracing::info!("Tick");
    //     let _ = network.publish("messages", k.clone()).await;

    //     let res = network
    //         .request(
    //             "12D3KooWH3uVF6wv47WnArKHk5p6cvgCJEb74UTmxztmQDc298L3".parse()?,
    //             Request::Work(),
    //         )
    //         .await;

    //     tracing::info!(response=?res,"Got a response");
    // }

    // Ok(())
}
