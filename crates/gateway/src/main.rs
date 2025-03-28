mod network;

use std::error::Error;

use futures::{AsyncReadExt, StreamExt};

use hypha_network::kad::KademliaInterface;
use tokio::sync::oneshot;
use tokio::task;
use tokio::task::JoinHandle;
use tokio::task::LocalSet;

use clap::Parser;

use tracing_subscriber::EnvFilter;

use hypha_network::listen::ListenInterface;
use hypha_network::stream::StreamReceiverInterface;
use hypha_network::swarm::SwarmDriver;
use hypha_network::swarm::SwarmInterface;
use hypha_network::utils::generate_ed25519;

use crate::network::Network;

#[derive(Debug, Parser)]
#[clap(name = "hypha")]
struct Opt {
    #[clap(long)]
    secret_key_seed: u8,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let opt = Opt::parse();

    tracing::info!(
        "Starting worker with secret key seed {}",
        opt.secret_key_seed
    );

    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let opt = Opt::parse();

    let local_key = generate_ed25519(opt.secret_key_seed).expect("only errors on wrong length");

    let (network, network_driver) = Network::create(local_key)?;

    // Create a LocalSet
    let local = LocalSet::new();

    // Spawn the NetworkDriver inside the LocalSet
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    local.spawn_local(async move {
        match tokio::select! {
            result = network_driver.run() => result,
            _ = shutdown_rx => {
                tracing::info!("Network shutdown requested");
                Ok(())
            }
        } {
            Ok(()) => tracing::info!("Network driver shut down gracefully"),
            Err(e) => tracing::error!("Network runner error: {:?}", e),
        }
    });

    let main_task: tokio::task::JoinHandle<Result<(), Box<dyn Error + Send + Sync>>> =
        tokio::spawn(async move {
            let _ = network
                .listen("/ip4/0.0.0.0/udp/0000/quic-v1".parse()?)
                .await;
            let _ = network.listen("/ip4/0.0.0.0/tcp/0000".parse()?).await;
            tracing::info!("Successfully listening");

            let _ = network.provide("test").await;

            let mut streams = network.streams()?;

            while let Some((peer, mut stream)) = streams.next().await {
                let _: JoinHandle<Result<(), std::io::Error>> = task::spawn(async move {
                    let mut buf = [0u8; 100];
                    let mut total = 0;
                    loop {
                        let read = stream.read(&mut buf).await?;
                        if read == 0 {
                            break;
                        }

                        total += read;

                        // We can also write responses back.
                        // stream.write_all(&buf[..read]).await?;
                    }

                    tracing::info!(bytes=total, peer=%peer, "Recieved");
                    Ok(())
                });
            }

            // Wait for Ctrl+C signal
            tokio::signal::ctrl_c().await?;

            // Signal shutdown
            tracing::info!("Shutting down...");
            let _ = shutdown_tx.send(());

            Ok(())
        });

    tokio::select! {
        _ = local => {
            tracing::info!("LocalSet completed");
        },
        result = main_task => {
            match result {
                Ok(inner_result) => {
                    if let Err(e) = inner_result {
                        tracing::error!("Main task error: {:?}", e);
                    }
                },
                Err(e) => {
                    tracing::error!("Main task panicked: {:?}", e);
                }
            }
        }
    }

    tracing::info!("Shutdown complete");
    Ok(())
}
