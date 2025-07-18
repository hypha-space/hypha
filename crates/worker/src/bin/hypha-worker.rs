//! Worker binary.

use std::{error::Error, fs, net::SocketAddr, path::PathBuf, time::Duration};

use clap::{Parser, ValueEnum};
use futures_util::{AsyncReadExt, AsyncWriteExt, StreamExt};
use hypha_network::{
    cert::{load_certs_from_pem, load_crls_from_pem, load_private_key_from_pem},
    dial::DialInterface,
    listen::ListenInterface,
    stream::{StreamReceiverInterface, StreamSenderInterface},
    swarm::SwarmDriver,
    utils::multiaddr_from_socketaddr,
};
use hypha_worker::network::Network;
use libp2p::{PeerId, multiaddr::Protocol};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Clone, Debug, ValueEnum)]
enum Role {
    Worker,
    ParameterServer,
}

#[derive(Debug, Parser)]
#[command(
    name = "hypha-worker",
    version,
    about = "Hypha Worker Node",
    long_about = "Runs a Hypha Worker which executes tasks.",
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
    #[clap(long)]
    gateway_address: SocketAddr,
    #[clap(long, default_value = "[::1]:0")]
    listen_address: SocketAddr,
    /// Socket to use for driver communication.
    #[clap(long, default_value = "/tmp/hypha.sock")]
    socket: PathBuf,
    /// Role of the worker in DiLoCo tasks.
    #[arg(value_enum)]
    role: Role,
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

    let gateway_address = multiaddr_from_socketaddr(opt.gateway_address)?;

    let (network, network_driver) = Network::create(cert_chain, private_key, ca_certs, crls)?;
    let network_driver_handle = tokio::spawn(network_driver.run());

    network
        .listen(multiaddr_from_socketaddr(opt.listen_address)?)
        .await?;
    tracing::info!("Successfully listening");

    // Dial the gateway address
    let gateway_peer_id = network.dial(gateway_address.clone()).await?;

    tracing::info!(gateway_id = %gateway_peer_id, "Connected to gateway");
    // Wait a bit until DHT bootstrapping is done.
    // Once we receive an 'Identify' message, bootstrapping will start.
    // TODO: Provide a way to wait for this event
    tokio::time::sleep(Duration::from_secs(5)).await;

    network
        .listen(
            gateway_address
                .with_p2p(gateway_peer_id)
                .unwrap()
                .with(Protocol::P2pCircuit),
        )
        .await?;

    tracing::info!(gateway_id = %gateway_peer_id, "p2p circuit setup");

    let cancellation_token = CancellationToken::new();

    let mut stream = network.streams()?;

    let stream_token = cancellation_token.clone();
    let stream_handle = tokio::spawn(async move {
        tokio::select! {
            _ = stream_token.cancelled() => {
                tracing::info!("Stream handler cancelled");
            }
            _ = async {
                while let Some((peer_id, mut stream)) = stream.next().await {
                    tracing::info!(peer_id = %peer_id, "Openend stream");

                    let mut buf = Vec::new();
                    stream.read_to_end(&mut buf).await.expect("read stream");
                    tracing::info!(peer_id = %peer_id, "Recieved {buf:?}");
                }
            } => {}
        }
    });

    let network_clone = network.clone();
    let stdin_token = cancellation_token.clone();
    let stdin_handle = tokio::spawn(async move {
        tracing::info!("Starting stdin handle");
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin);

        loop {
            let mut input = String::new();
            tokio::select! {
                _ = stdin_token.cancelled() => {
                    tracing::info!("Stdin handler cancelled");
                    break;
                }
                result = reader.read_line(&mut input) => {
                    match result {
                        Ok(0) => break, // EOF
                        Ok(_) => {
                    tracing::info!("Received input: {}", input);
                    let input = input.trim();
                    if input.is_empty() {
                        continue;
                    }

                    // Parse input as "peer_id message"
                    let parts: Vec<&str> = input.splitn(2, ' ').collect();
                    if parts.len() != 2 {
                        tracing::error!("Invalid input format. Expected: <peer_id> <message>");
                        continue;
                    }

                    let peer_id_str = parts[0];
                    let message = parts[1];

                    // Parse peer ID
                    let peer_id = match peer_id_str.parse::<PeerId>() {
                        Ok(id) => id,
                        Err(e) => {
                            tracing::error!("Invalid peer ID '{}': {}", peer_id_str, e);
                            continue;
                        }
                    };

                    // Open stream to peer and send message
                    match network_clone.stream(peer_id).await {
                        Ok(mut stream) => {
                            if let Err(e) = stream.write_all(message.as_bytes()).await {
                                tracing::error!(
                                    "Failed to send message to peer {}: {}",
                                    peer_id,
                                    e
                                );
                            } else {
                                tracing::info!("Sent message to peer {}: {}", peer_id, message);
                            }
                            if let Err(e) = stream.close().await {
                                tracing::error!(
                                    peer_id=%peer_id,
                                    error=?e,
                                    "Failed to close stream"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to open stream to peer {}: {}", peer_id, e);
                        }
                    }
                }
                        Err(e) => {
                            tracing::error!("Failed to read from stdin: {}", e);
                            break;
                        }
                    }
                }
            }
        }
    });

    let mut sigterm = signal(SignalKind::terminate())?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Received SIGINT, shutting down");
        }
        _ = sigterm.recv() => {
            tracing::info!("Received SIGTERM, shutting down");
        }
        // All of these futures handle streams of events.
        // If any of them terminate on their own, it must be due to an error.
        // We warn about that, then shut down gracefully.
        // TODO: Log errors if futures return one.
        _ = network_driver_handle => {
            tracing::warn!("Network driver error, shutting down");
        }

        _ = stream_handle => {
            tracing::warn!("Stream handler error, shutting down");
        }

        _ = stdin_handle => {
            tracing::info!("Stdin handler finished, shutting down");
        }
    }

    // Cancel all running tasks
    cancellation_token.cancel();

    tokio::time::sleep(Duration::from_millis(100)).await;

    Ok(())
}
