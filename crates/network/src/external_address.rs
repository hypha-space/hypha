//! Utilities for managing external addresses.
//!
//! Components use this module to configure and manage external addresses that
//! should be advertised to peers. It handles automatic detection through the
//! identify protocol and manual configuration.

use libp2p::{Multiaddr, swarm::NetworkBehaviour};

use crate::swarm::SwarmDriver;

/// Actions that can be sent to an external address driver for processing.
///
/// These actions represent external address operations that need to be handled
/// by the network event loop.
pub enum ExternalAddressAction {
    /// Add an external address.
    ///
    /// This address will be advertised to other peers in the network.
    AddExternalAddress(Multiaddr),
}

/// Trait for handling external address management within a swarm driver.
///
/// This trait extends [`SwarmDriver`] to provide external address functionality.
/// It handles identify events and can automatically add observed addresses as
/// external addresses when appropriate.
pub trait ExternalAddressDriver<TBehavior>: SwarmDriver<TBehavior> + Send
where
    TBehavior: NetworkBehaviour,
{
    /// Process an external address action.
    ///
    /// # Arguments
    ///
    /// * `action` - The external address action to process
    fn process_external_address_action(
        &mut self,
        action: ExternalAddressAction,
    ) -> impl Future<Output = ()> + Send {
        async move {
            match action {
                ExternalAddressAction::AddExternalAddress(address) => {
                    tracing::debug!(address = %address, "Adding external address");
                    self.swarm().add_external_address(address.clone());
                }
            }
        }
    }
}

/// Interface for sending external address actions to a network driver.
///
/// This trait provides a high-level API for managing external addresses
/// without needing to interact directly with the libp2p swarm.
///
/// # Examples
///
/// ```rust,no_run
/// use hypha_network::external_address::ExternalAddressInterface;
/// use libp2p::Multiaddr;
///
/// async fn example_usage(network: impl ExternalAddressInterface) {
///     // Add an external address
///     let addr = "/ip4/1.2.3.4/tcp/8080".parse().unwrap();
///     network.add_external_address(addr).await;
/// }
/// ```
pub trait ExternalAddressInterface: Sync {
    /// Send an external address action to the network driver.
    ///
    /// This is the low-level method for sending actions. Most users should
    /// prefer the higher-level [`add_external_address`](Self::add_external_address) method.
    fn send(&self, action: ExternalAddressAction) -> impl Future<Output = ()> + Send;

    /// Add an external address.
    ///
    /// This address will be advertised to other peers in the network.
    /// Use this when you know your public IP/address (e.g., from configuration
    /// or an external service) to ensure proper connectivity.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hypha_network::external_address::ExternalAddressInterface;
    /// # use libp2p::Multiaddr;
    /// # async fn example(network: impl ExternalAddressInterface) {
    /// // Add a public IPv4 address
    /// let addr = "/ip4/203.0.113.1/tcp/8080".parse().unwrap();
    /// network.add_external_address(addr).await;
    ///
    /// // Add a DNS address
    /// let dns_addr = "/dns4/example.com/tcp/8080".parse().unwrap();
    /// network.add_external_address(dns_addr).await;
    /// # }
    /// ```
    fn add_external_address(&self, address: Multiaddr) -> impl Future<Output = ()> + Send {
        async move {
            self.send(ExternalAddressAction::AddExternalAddress(address))
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use mockall::mock;

    use super::*;

    mock! {
        TestInterface {}

        impl ExternalAddressInterface for TestInterface {
            async fn send(&self, action: ExternalAddressAction);
        }
    }

    #[tokio::test]
    async fn test_add_external_address() {
        let mut mock = MockTestInterface::new();
        let addr: Multiaddr = "/ip4/1.2.3.4/tcp/8080".parse().unwrap();
        let addr_clone = addr.clone();

        mock.expect_send()
            .withf(move |action| {
                matches!(
                    action,
                    ExternalAddressAction::AddExternalAddress(a)
                    if a == &addr_clone
                )
            })
            .times(1)
            .returning(|_| ());

        mock.add_external_address(addr).await;
    }
}
