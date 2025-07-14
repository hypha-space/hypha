//! Handles outbound dialing to peers.
//!
//! This module abstracts the details of initiating connections and tracking
//! pending dials. It provides traits for dialing behavior implementations
//! and helpers to manage asynchronous dialing responses.

use std::collections::HashMap;

use libp2p::{
    Multiaddr, PeerId,
    swarm::{ConnectionId, DialError, NetworkBehaviour, dial_opts::DialOpts},
};
use tokio::sync::oneshot;

use crate::swarm::SwarmDriver;

/// Type alias for tracking pending dial operations.
///
/// Maps connection IDs to response channels that will receive the dial result
/// once the connection attempt completes (either successfully or with an error).
pub type PendingDials = HashMap<ConnectionId, oneshot::Sender<Result<PeerId, DialError>>>;

/// Actions that can be sent to a dial driver for processing.
///
/// These actions represent dial-related operations that need to be handled
/// by the network event loop. Each action includes the necessary data and
/// a response channel to communicate the result back to the caller.
pub enum DialAction {
    /// Initiate a dial to the specified multiaddress.
    ///
    /// The sender will receive the result of the dial attempt, containing
    /// either the successfully connected peer ID or a dial error.
    Dial(Multiaddr, oneshot::Sender<Result<PeerId, DialError>>),
}

/// Trait for handling outbound dial operations within a swarm driver.
///
/// This trait extends [`SwarmDriver`] to provide dial-specific functionality.
/// Implementations should track pending dials and process dial-related events
/// from the libp2p swarm.
pub trait DialDriver<TBehavior>: SwarmDriver<TBehavior> + Send
where
    TBehavior: NetworkBehaviour,
{
    /// Returns a mutable reference to the pending dials tracker.
    ///
    /// This is used to store and retrieve dial response channels while
    /// connection attempts are in progress.
    fn pending_dials(&mut self) -> &mut PendingDials;

    /// Process a dial action by initiating the connection attempt.
    ///
    /// This method handles [`DialAction::Dial`] by calling the libp2p swarm's
    /// dial method and tracking the connection attempt for later completion.
    ///
    /// # Arguments
    ///
    /// * `action` - The dial action to process
    fn process_dial_action(&mut self, action: DialAction) -> impl Future<Output = ()> + Send {
        async move {
            match action {
                DialAction::Dial(address, tx) => {
                    tracing::info!(address=%address.clone(),"Dialing");
                    let opts = DialOpts::from(address);
                    let connection_id = opts.connection_id();

                    if let Err(err) = self.swarm().dial(opts) {
                        let _ = tx.send(Err(err));
                    } else {
                        self.pending_dials().insert(connection_id, tx);
                    }
                }
            }
        }
    }

    /// Handle successful connection establishment.
    ///
    /// This should be called when a libp2p `ConnectionEstablished` event
    /// is received. It notifies any pending dial operations that the
    /// connection was successful.
    ///
    /// # Arguments
    ///
    /// * `peer_id` - The peer ID of the successfully connected peer
    /// * `connection_id` - The connection ID of the established connection
    fn process_connection_established(
        &mut self,
        peer_id: PeerId,
        connection_id: &ConnectionId,
    ) -> impl Future<Output = ()> + Send {
        async move {
            if let Some(dial) = self.pending_dials().remove(connection_id) {
                let _ = dial.send(Ok(peer_id));
            }
        }
    }

    /// Handle connection establishment errors.
    ///
    /// This should be called when a libp2p dial fails. It notifies any
    /// pending dial operations about the failure.
    ///
    /// # Arguments
    ///
    /// * `connection_id` - The connection ID of the failed connection attempt
    /// * `error` - The dial error that occurred
    fn process_connection_error(
        &mut self,
        connection_id: &ConnectionId,
        error: DialError,
    ) -> impl Future<Output = ()> + Send {
        async move {
            if let Some(dial) = self.pending_dials().remove(connection_id) {
                let _ = dial.send(Err(error));
            }
        }
    }
}

/// Interface for sending dial actions to a network driver.
///
/// This trait provides a high-level API for dialing peers without needing
/// to interact directly with the libp2p swarm. Implementations typically
/// send actions through a channel to the network event loop.
///
/// # Examples
///
/// ```rust,no_run
/// use hypha_network::dial::{DialInterface, DialAction};
/// use libp2p::Multiaddr;
/// use tokio::sync::mpsc;
///
/// struct NetworkInterface {
///     action_sender: mpsc::Sender<DialAction>,
/// }
///
/// impl DialInterface for NetworkInterface {
///     async fn send(&self, action: DialAction) {
///         let _ = self.action_sender.send(action).await;
///     }
/// }
///
/// async fn example_usage(network: &NetworkInterface) {
///     let addr = "/ip4/127.0.0.1/tcp/1234".parse().unwrap();
///     match network.dial(addr).await {
///         Ok(peer_id) => println!("Connected to peer: {}", peer_id),
///         Err(e) => println!("Dial failed: {}", e),
///     }
/// }
/// ```
pub trait DialInterface: Sync {
    /// Send a dial action to the network driver.
    ///
    /// This is the low-level method for sending actions. Most users should
    /// prefer the higher-level [`dial`](Self::dial) method.
    fn send(&self, action: DialAction) -> impl Future<Output = ()> + Send;

    /// Dial a peer at the specified multiaddress.
    ///
    /// This method initiates a connection attempt to the given address and
    /// waits for the result. It's a convenience wrapper around sending a
    /// [`DialAction::Dial`] and waiting for the response.
    ///
    /// # Arguments
    ///
    /// * `address` - The multiaddress to dial
    ///
    /// # Returns
    ///
    /// * `Ok(PeerId)` - The peer ID of the successfully connected peer
    /// * `Err(DialError)` - The error that occurred during dialing
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hypha_network::dial::DialInterface;
    /// # async fn example(network: impl DialInterface) -> Result<(), libp2p::swarm::DialError> {
    /// let addr = "/ip4/127.0.0.1/tcp/1234".parse().unwrap();
    /// let peer_id = network.dial(addr).await?;
    /// println!("Connected to: {}", peer_id);
    /// # Ok(())
    /// # }
    /// ```
    fn dial(&self, address: Multiaddr) -> impl Future<Output = Result<PeerId, DialError>> + Send {
        async move {
            let (tx, rx) = oneshot::channel();
            tracing::info!(address=%address.clone(),"Dialing");

            self.send(DialAction::Dial(address, tx)).await;

            rx.await.map_err(|_| DialError::Aborted)?
        }
    }
}

#[cfg(test)]
mod dial_interface_tests {
    use libp2p::{Multiaddr, PeerId, swarm::DialError};
    use mockall::mock;

    use super::*;

    mock! {
        NetworkInterface {}

        impl DialInterface for NetworkInterface {
            async fn send(&self, action: DialAction);
        }
    }

    #[tokio::test]
    async fn test_dial_interface_success() {
        let mut mock = MockNetworkInterface::new();
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/1234".parse().unwrap();
        let addr_clone = addr.clone();

        mock.expect_send()
            .withf(move |action| matches!(action, DialAction::Dial(a, _) if a == &addr_clone))
            .times(1)
            .returning(|action| {
                let DialAction::Dial(_, tx) = action;
                tokio::spawn(async move {
                    let peer = PeerId::random();
                    let _ = tx.send(Ok(peer));
                });
            });

        // calling the real `.dial` on our mock
        let result = mock.dial(addr.clone()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_dial_interface_aborted() {
        let mut mock = MockNetworkInterface::new();
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/1234".parse().unwrap();
        let addr_clone = addr.clone();

        mock.expect_send()
            .withf(move |action| matches!(action, DialAction::Dial(a, _) if a == &addr_clone))
            .times(1)
            .returning(|_action| {});

        let err = mock.dial(addr.clone()).await.unwrap_err();
        assert!(matches!(err, DialError::Aborted));
    }
}
