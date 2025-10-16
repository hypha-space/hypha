//! Utilities for listening on network addresses.
//!
//! Components use this module to manage listeners and to accept incoming
//! connections. It defines actions for starting listeners and channels for
//! reporting back the results.

use std::{collections::HashMap, io::Error as IoError};

use libp2p::{Multiaddr, TransportError, core::transport::ListenerId, swarm::NetworkBehaviour};
use tokio::sync::oneshot;

use crate::swarm::SwarmDriver;

/// Type alias for tracking pending listen operations.
///
/// Maps listener IDs to response channels that will receive the listen result
/// once the listener is established (either successfully or with an error).
pub type PendingListens = HashMap<ListenerId, oneshot::Sender<Result<(), TransportError<IoError>>>>;

/// Actions that can be sent to a listen driver for processing.
///
/// These actions represent listen-related operations that need to be handled
/// by the network event loop. Each action includes the necessary data and
/// a response channel to communicate the result back to the caller.
pub enum ListenAction {
    /// Start listening on the specified multiaddress.
    ///
    /// The sender will receive the result of the listen attempt, indicating
    /// whether the listener was successfully established or if an error occurred.
    Listen(
        Multiaddr,
        oneshot::Sender<Result<(), TransportError<IoError>>>,
    ),
}

/// Trait for handling inbound listen operations within a swarm driver.
///
/// This trait extends [`SwarmDriver`] to provide listen-specific functionality.
/// Implementations should track pending listeners and process listen-related
/// events from the libp2p swarm.
pub trait ListenDriver<TBehavior>: SwarmDriver<TBehavior> + Send
where
    TBehavior: NetworkBehaviour,
{
    /// Returns a mutable reference to the pending listens tracker.
    ///
    /// This is used to store and retrieve listen response channels while
    /// listener establishment is in progress.
    fn pending_listens(&mut self) -> &mut PendingListens;

    /// Process a listen action by starting a listener on the specified address.
    ///
    /// This method handles [`ListenAction::Listen`] by calling the libp2p swarm's
    /// listen_on method and tracking the listener for later completion.
    ///
    /// # Arguments
    ///
    /// * `action` - The listen action to process
    fn process_listen_action(&mut self, action: ListenAction) -> impl Future<Output = ()> + Send {
        async move {
            match action {
                ListenAction::Listen(address, tx) => {
                    match self.swarm().listen_on(address.clone()) {
                        Ok(listener_id) => {
                            self.pending_listens().insert(listener_id, tx);
                        }
                        Err(err) => {
                            let _ = tx.send(Err(err));
                        }
                    }
                }
            }
        }
    }

    /// Handle successful listener establishment.
    ///
    /// This should be called when a libp2p `NewListenAddr` event is received.
    /// It notifies any pending listen operations that the listener was
    /// successfully established.
    ///
    /// # Arguments
    ///
    /// * `listener_id` - The listener ID of the successfully established listener
    fn process_new_listen_addr(
        &mut self,
        listener_id: &ListenerId,
        address: Multiaddr,
    ) -> impl Future<Output = ()> + Send {
        tracing::info!(address=%address.clone(),"Listening");

        async move {
            if let Some(tx) = self.pending_listens().remove(listener_id) {
                let _ = tx.send(Ok(()));
            }
        }
    }
}

/// Interface for sending listen actions to a network driver.
///
/// This trait provides a high-level API for starting listeners without needing
/// to interact directly with the libp2p swarm. Implementations typically
/// send actions through a channel to the network event loop.
///
/// # Examples
///
/// ```rust,no_run
/// use hypha_network::listen::{ListenInterface, ListenAction};
/// use libp2p::Multiaddr;
/// use tokio::sync::mpsc;
///
/// struct NetworkServer {
///     action_sender: mpsc::Sender<ListenAction>,
/// }
///
/// impl ListenInterface for NetworkServer {
///     async fn send(&self, action: ListenAction) {
///         let _ = self.action_sender.send(action).await;
///     }
/// }
///
/// async fn example_usage(server: &NetworkServer) {
///     let addr = "/ip4/0.0.0.0/tcp/1234".parse().unwrap();
///     match server.listen(addr).await {
///         Ok(()) => println!("Successfully started listening"),
///         Err(e) => println!("Listen failed: {}", e),
///     }
/// }
/// ```
pub trait ListenInterface: Sync {
    /// Send a listen action to the network driver.
    ///
    /// This is the low-level method for sending actions. Most users should
    /// prefer the higher-level [`listen`](Self::listen) method.
    fn send(&self, action: ListenAction) -> impl Future<Output = ()> + Send;

    /// Start listening on the specified multiaddress.
    ///
    /// This method initiates a listener on the given address and waits for
    /// the result. It's a convenience wrapper around sending a
    /// [`ListenAction::Listen`] and waiting for the response.
    ///
    /// # Arguments
    ///
    /// * `address` - The multiaddress to listen on
    ///
    /// # Returns
    ///
    /// * `Ok(())` - The listener was successfully established
    /// * `Err(TransportError)` - The error that occurred during listener setup
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hypha_network::listen::ListenInterface;
    /// # async fn example(network: impl ListenInterface) -> Result<(), libp2p::TransportError<std::io::Error>> {
    /// let addr = "/ip4/0.0.0.0/tcp/1234".parse().unwrap();
    /// network.listen(addr).await?;
    /// println!("Listening for connections...");
    /// # Ok(())
    /// # }
    /// ```
    fn listen(
        &self,
        address: Multiaddr,
    ) -> impl Future<Output = Result<(), TransportError<IoError>>> + Send {
        async move {
            let (tx, rx) = oneshot::channel();

            self.send(ListenAction::Listen(address, tx)).await;

            rx.await.map_err(|err| {
                TransportError::Other(IoError::other(format!(
                    "Failed to recieve action response: {err}"
                )))
            })?
        }
    }
}

#[cfg(test)]
mod listen_interface_tests {
    use libp2p::{Multiaddr, core::transport::TransportError};
    use mockall::mock;

    use super::*;

    mock! {
        TestListenInterface {}

        impl ListenInterface for TestListenInterface {
            async fn send(&self, action: ListenAction);
        }
    }

    #[tokio::test]
    async fn test_listen_interface_success() {
        let mut mock = MockTestListenInterface::new();
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/4321".parse().unwrap();
        let addr_clone = addr.clone();

        mock.expect_send()
            .withf(move |action| matches!(action, ListenAction::Listen(a, _) if a == &addr_clone))
            .times(1)
            .returning(|action| match action {
                ListenAction::Listen(_, tx) => {
                    tokio::spawn(async move {
                        let _ = tx.send(Ok(()));
                    });
                }
            });

        let result = mock.listen(addr.clone()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_listen_interface_aborted() {
        let mut mock = MockTestListenInterface::new();
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/4321".parse().unwrap();
        let addr_clone = addr.clone();

        mock.expect_send()
            .withf(move |action| matches!(action, ListenAction::Listen(a, _) if a == &addr_clone))
            .times(1)
            .returning(|_action| {});

        let err = mock.listen(addr.clone()).await.unwrap_err();

        match err {
            TransportError::Other(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::Other);
            }
            _ => panic!("Expected Other error"),
        }
    }
}
