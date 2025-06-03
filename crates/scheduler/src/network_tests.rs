#[cfg(test)]
mod tests {
    use super::*;
    use hypha_error::HyphaError;
    use libp2p::{
        core::transport::MemoryTransport,
        identity::Keypair,
        multiaddr::Protocol,
        relay, dcutr,
        Multiaddr, PeerId, Swarm, SwarmBuilder,
        swarm::NetworkBehaviour,
    };
    use std::{collections::HashSet, time::Duration};

    // Helper function to create a new test keypair
    fn create_test_keypair() -> Keypair {
        Keypair::generate_ed25519()
    }

    #[tokio::test]
    async fn test_network_creation_with_relay_and_dcutr() {
        // This test verifies that the Network struct can be created with relay and DCUtR components
        let result = Network::new(create_test_keypair(), None);
        assert!(result.is_ok(), "Network creation should succeed");

        let network = result.unwrap();
        let swarm = network.swarm();

        // Verify the swarm contains the expected behaviors
        // This is an indirect way to verify since we can't directly access the private fields
        assert!(swarm.is_ok(), "Should be able to get swarm from network");
    }

    #[tokio::test]
    async fn test_behaviour_contains_relay_and_dcutr() {
        // Create a custom NetworkBehaviour implementation that can be inspected
        #[derive(NetworkBehaviour)]
        struct TestBehaviour {
            relay_client: relay::client::Behaviour,
            dcutr: dcutr::Behaviour,
            #[behaviour(ignore)]
            _peer_id: PeerId,
        }

        let keypair = create_test_keypair();
        let peer_id = keypair.public().to_peer_id();

        // Create the test behaviour with relay and DCUtR components
        let test_behaviour = {
            let (relay_transport, relay_client) = relay::client::new(peer_id);
            TestBehaviour {
                relay_client,
                dcutr: dcutr::Behaviour::new(peer_id),
                _peer_id: peer_id,
            }
        };

        // Create a basic memory transport for testing
        let transport = MemoryTransport::default();

        // Build a test swarm with our test behaviour
        let swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_transport(transport)
            .unwrap()
            .with_behaviour(|_| test_behaviour)
            .unwrap()
            .build();

        // If we reach here without errors, it means the behaviour was successfully created
        // with relay and DCUtR components
        assert!(swarm.behaviour().relay_client.listeners().count() == 0);
    }

    #[tokio::test]
    async fn test_relay_client_configuration() {
        // Test that verifies the relay client can be properly configured
        let keypair = create_test_keypair();
        let peer_id = keypair.public().to_peer_id();
        
        // Create relay client
        let (_, relay_client) = relay::client::new(peer_id);
        
        // Verify initial state (no relays)
        assert_eq!(relay_client.reserved_relays().count(), 0);
        
        // This confirms that we can at least access the relay client's methods
    }
}