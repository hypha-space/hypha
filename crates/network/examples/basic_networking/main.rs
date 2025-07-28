use std::{collections::HashMap, time::Duration};

use clap::{Parser, Subcommand};
use futures::StreamExt;
use hypha_network::{
    dial::{DialAction, DialDriver, DialInterface, PendingDials},
    error::HyphaError,
    listen::{ListenAction, ListenDriver, ListenInterface, PendingListens},
    swarm::SwarmDriver,
};
use libp2p::{
    Multiaddr, Swarm, ping,
    swarm::{NetworkBehaviour, SwarmEvent},
};
use libp2p_swarm_test::SwarmExt;
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[clap(
    name = "basic-networking",
    bin_name = "cargo run --example basic_networking",
    about = "Example of basic networking with dial/listen",
    version
)]
struct Args {
    #[clap(subcommand)]
    variant: Variants,
}

#[derive(Subcommand, Clone)]
enum Variants {
    Server {
        #[clap(long, default_value = "/memory/server")]
        listen_addr: String,
    },
    Client {
        #[clap(long)]
        server_addr: String,
    },
    Demo,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    match args.variant.clone() {
        Variants::Server { listen_addr } => server(&listen_addr).await?,
        Variants::Client { server_addr } => client(&server_addr).await?,
        Variants::Demo => demo().await?,
    }

    Ok(())
}

async fn server(listen_addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (network, driver) = create_network();
    tokio::spawn(driver.run());

    let listen_addr = listen_addr.parse::<Multiaddr>()?;
    network.listen(listen_addr.clone()).await?;
    tracing::info!("Server listening on: {}", listen_addr);

    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down server");
    Ok(())
}

async fn client(server_addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (network, driver) = create_network();
    tokio::spawn(driver.run());

    let server_addr = server_addr.parse::<Multiaddr>()?;
    tracing::info!("Attempting to connect to: {}", server_addr);

    match network.dial(server_addr).await {
        Ok(peer_id) => {
            tracing::info!("Successfully connected to peer: {}", peer_id);
        }
        Err(e) => {
            tracing::error!("Failed to connect: {}", e);
            return Err(e.into());
        }
    }

    tokio::time::sleep(Duration::from_secs(2)).await;
    Ok(())
}

async fn demo() -> Result<(), Box<dyn std::error::Error>> {
    tracing::info!("Running basic networking demo");

    // Create two networks
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    // Set up memory addresses for testing
    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    // Connect them using libp2p test utilities
    swarm1.connect(&mut swarm2).await;

    let peer2_id = *swarm2.local_peer_id();

    // Create network interfaces
    let (_network1, driver1) = Network::create(swarm1);
    let (_network2, driver2) = Network::create(swarm2);

    // Start the drivers
    let handle1 = tokio::spawn(driver1.run());
    let handle2 = tokio::spawn(driver2.run());

    tracing::info!("Demo networks created and connected");
    tracing::info!("Peer 2 ID: {}", peer2_id);

    // Demonstrate error handling
    demo_error_handling().await;

    // Clean up
    handle1.abort();
    handle2.abort();

    tracing::info!("Demo completed successfully");
    Ok(())
}

async fn demo_error_handling() {
    tracing::info!("Demonstrating error handling");

    // Show different error types
    let dial_error = HyphaError::DialError("Connection failed".to_string());
    let swarm_error = HyphaError::SwarmError("Swarm initialization failed".to_string());

    tracing::info!("Dial error: {}", dial_error);
    tracing::info!("Swarm error: {}", swarm_error);

    // Show debug formatting
    tracing::debug!("Dial error debug: {:?}", dial_error);
    tracing::debug!("Swarm error debug: {:?}", swarm_error);
}

fn create_test_swarm() -> Swarm<Behaviour> {
    Swarm::new_ephemeral_tokio(|_| Behaviour {
        ping: ping::Behaviour::default(),
    })
}

fn create_network() -> (Network, NetworkDriver) {
    let swarm = create_test_swarm();
    Network::create(swarm)
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    ping: ping::Behaviour,
}

enum Action {
    Dial(DialAction),
    Listen(ListenAction),
}

struct NetworkDriver {
    swarm: Swarm<Behaviour>,
    pending_dials: PendingDials,
    pending_listens: PendingListens,
    action_rx: mpsc::Receiver<Action>,
}

impl SwarmDriver<Behaviour> for NetworkDriver {
    fn swarm(&mut self) -> &mut Swarm<Behaviour> {
        &mut self.swarm
    }

    async fn run(mut self) -> Result<(), HyphaError> {
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::ConnectionEstablished { peer_id, connection_id, .. } => {
                            tracing::info!("Connected to {}", peer_id);
                            self.process_connection_established(peer_id, &connection_id).await;
                        }
                        SwarmEvent::OutgoingConnectionError { connection_id, error, .. } => {
                            tracing::error!("Connection error: {}", error);
                            self.process_connection_error(&connection_id, error).await;
                        }
                        SwarmEvent::NewListenAddr { address, .. } => {
                            tracing::info!("Listening on {}", address);
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Ping(event)) => {
                            tracing::debug!("Ping event: {:?}", event);
                        }
                        _ => {}
                    }
                }
                Some(action) = self.action_rx.recv() => {
                    match action {
                        Action::Dial(action) => self.process_dial_action(action).await,
                        Action::Listen(action) => self.process_listen_action(action).await,
                    }
                }
                else => break
            }
        }

        Ok(())
    }
}

impl DialDriver<Behaviour> for NetworkDriver {
    fn pending_dials(&mut self) -> &mut PendingDials {
        &mut self.pending_dials
    }
}

impl ListenDriver<Behaviour> for NetworkDriver {
    fn pending_listens(&mut self) -> &mut PendingListens {
        &mut self.pending_listens
    }
}

#[derive(Clone)]
struct Network {
    action_tx: mpsc::Sender<Action>,
}

impl Network {
    fn create(swarm: Swarm<Behaviour>) -> (Self, NetworkDriver) {
        let (action_tx, action_rx) = mpsc::channel(100);

        let driver = NetworkDriver {
            swarm,
            pending_dials: HashMap::new(),
            pending_listens: HashMap::new(),
            action_rx,
        };

        let interface = Self { action_tx };

        (interface, driver)
    }
}

impl DialInterface for Network {
    async fn send(&self, action: DialAction) {
        let _ = self.action_tx.send(Action::Dial(action)).await;
    }
}

impl ListenInterface for Network {
    async fn send(&self, action: ListenAction) {
        let _ = self.action_tx.send(Action::Listen(action)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_connection() {
        let mut swarm1 = create_test_swarm();
        let mut swarm2 = create_test_swarm();

        swarm1.listen().with_memory_addr_external().await;
        swarm2.listen().with_memory_addr_external().await;

        swarm1.connect(&mut swarm2).await;

        let peer2 = *swarm2.local_peer_id();

        let (network1, driver1) = Network::create(swarm1);
        let (_network2, driver2) = Network::create(swarm2);

        let handle1 = tokio::spawn(driver1.run());
        let handle2 = tokio::spawn(driver2.run());

        // Test should complete without errors
        tokio::time::sleep(Duration::from_millis(100)).await;

        handle1.abort();
        handle2.abort();
    }

    #[test]
    fn test_error_types() {
        let dial_error = HyphaError::DialError("test".to_string());
        let swarm_error = HyphaError::SwarmError("test".to_string());

        // Test display
        assert_eq!(dial_error.to_string(), "Dial error: test");
        assert_eq!(swarm_error.to_string(), "Swarm error: test");

        // Test debug
        assert!(format!("{:?}", dial_error).contains("DialError"));
        assert!(format!("{:?}", swarm_error).contains("SwarmError"));
    }
}
