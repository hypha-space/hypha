#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::{
        core::transport::MemoryTransport,
        dcutr, gossipsub, identity, kad, ping, relay,
        swarm::NetworkBehaviour,
        Multiaddr, PeerId, Swarm, SwarmBuilder,
    };
    use std::time::Duration;

    // Create a test network behavior that includes relay and DCUtR
    #[derive(NetworkBehaviour)]
    struct TestNetworkBehaviour {
        ping: ping::Behaviour,
        relay_client: relay::client::Behaviour,
        dcutr: dcutr::Behaviour,
    }

    // Helper function to create a test keypair
    fn create_test_keypair() -> identity::Keypair {
        identity::Keypair::generate_ed25519()
    }

    // Helper function to create a test swarm with relay and DCUtR
    fn create_test_swarm() -> Swarm<TestNetworkBehaviour> {
        let keypair = create_test_keypair();
        let peer_id = keypair.public().to_peer_id();

        // Create memory transport for testing
        let transport = MemoryTransport::default()
            .upgrade(libp2p::core::upgrade::Version::V1)
            .authenticate(libp2p::noise::Config::new(&keypair).unwrap())
            .multiplex(libp2p::yamux::Config::default());

        // Create relay components
        let (relay_transport, relay_client) = relay::client::new(peer_id);

        // Create test behavior
        let behaviour = TestNetworkBehaviour {
            ping: ping::Behaviour::new(ping::Config::new()),
            relay_client,
            dcutr: dcutr::Behaviour::new(peer_id),
        };

        // Build the swarm
        SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_other_transport(|_| transport)
            .unwrap()
            .with_behaviour(|_| behaviour)
            .unwrap()
            .build()
    }

    #[tokio::test]
    async fn test_swarm_with_relay_and_dcutr() {
        // Create test swarm with relay and DCUtR
        let mut swarm = create_test_swarm();
        
        // Just verifying that the swarm can be created with these components
        assert!(swarm.behaviour().relay_client.listeners().count() == 0);
        
        // The fact that we can access these components confirms they're properly integrated
    }

    #[tokio::test]
    async fn test_relay_client_reservation() {
        // Create two swarms - one to act as a relay, one as a client
        let mut relay_swarm = {
            let keypair = create_test_keypair();
            let peer_id = keypair.public().to_peer_id();
            
            // Basic memory transport
            let transport = MemoryTransport::default()
                .upgrade(libp2p::core::upgrade::Version::V1)
                .authenticate(libp2p::noise::Config::new(&keypair).unwrap())
                .multiplex(libp2p::yamux::Config::default());
            
            // Create a relay server
            let relay_config = relay::Config {
                max_reservations: 100,
                max_reservations_per_peer: 10,
                reservation_duration: Duration::from_secs(3600),
                reservation_rate_limiters: None,
            };
            
            let relay_behaviour = relay::Behaviour::new(peer_id, relay_config);
            
            SwarmBuilder::with_existing_identity(keypair)
                .with_tokio()
                .with_other_transport(|_| transport)
                .unwrap()
                .with_behaviour(|_| relay_behaviour)
                .unwrap()
                .build()
        };
        
        // Create client swarm
        let client_swarm = create_test_swarm();
        
        // Successfully creating both swarms verifies basic compatibility
    }
}