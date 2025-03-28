mod network;

use std::{error::Error, sync::Arc};

use clap::Parser;
use hypha_network::{
    dial::DialInterface, kad::KademliaInterface, swarm::SwarmDriver, utils::generate_ed25519,
};
use libp2p::Multiaddr;
use tokio::{sync::oneshot, task::LocalSet};
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

    let (network_interface, network_driver) = Network::create(local_key)?;
    let network = Arc::new(network_interface);

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
            Err(e) => tracing::error!("Network driver error: {:?}", e),
        }
    });

    let main_task = tokio::spawn(async move {
        // Dial the gatewaty address
        let _peer_id = match network
            .dial(opt.gateway_address.parse::<Multiaddr>()?)
            .await
        {
            Ok(peer_id) => peer_id,
            Err(e) => {
                tracing::error!("Failed to dial hub: {:?}", e);
                return Err(Box::new(e) as Box<dyn Error + Send + Sync>);
            }
        };

        network
            .store(libp2p::kad::Record {
                key: libp2p::kad::RecordKey::new(&"cpu"),
                value: "test".as_bytes().to_vec(),
                publisher: None,
                expires: None,
            })
            .await?;
        tracing::info!("Stored 'cpu' record");

        network.provide("cpu").await?;
        tracing::info!("Provided 'cpu' service");

        // Stream data to gateway
        // let start_time = Instant::now();
        // let mut streams: Vec<JoinHandle<Result<(), Box<dyn Error + Send + Sync>>>> = Vec::new();
        // for i in 0..TASKS {
        //     let network_clone = Arc::clone(&network);
        //     streams.push(task::spawn(async move {
        //         tracing::info!(i=%i, "Starting stream");

        //         let mut stream = network_clone.stream(peer_id).await?;

        //         let bytes = vec![0u8; GIGABYTE / TASKS];

        //         stream.write_all(&bytes).await?;
        //         stream.close().await.ok();

        //         Ok(())
        //     }))
        // }

        // let _ = join_all(streams).await;

        // // Record the elapsed time after the transfer
        // let duration = start_time.elapsed();
        // tracing::info!("Transferred {:?} in {:?}", GIGABYTE, duration);

        // // Optionally, compute throughput (e.g., in MB/s)
        // let seconds = duration.as_secs_f64();
        // let throughput = (GIGABYTE as f64) / seconds / (1024.0 * 1024.0);
        // tracing::info!("Throughput: {:.2} MB/s", throughput);

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
