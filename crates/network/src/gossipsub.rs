//! Abstractions over the libp2p gossipsub protocol.
//!
//! This module exposes traits and helpers to publish and subscribe to topics
//! using gossipsub. It hides the underlying protocol details from the rest of
//! the code base and provides typed channels for incoming and outgoing
//! messages.

use std::collections::HashMap;

use libp2p::{PeerId, gossipsub, swarm::NetworkBehaviour};
use thiserror::Error;
use tokio::sync::{broadcast, oneshot};
use tokio_stream::wrappers::BroadcastStream;

use crate::swarm::SwarmDriver;

/// A message received through the gossipsub protocol.
///
/// This structure represents a message that has been received from a peer
/// in the gossipsub network. It contains the source peer ID and the raw
/// message data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GossipsubMessage {
    /// The peer ID of the message sender.
    pub source: PeerId,
    /// The raw message data as bytes.
    pub data: Vec<u8>,
}

/// Type alias for tracking active gossipsub subscriptions.
///
/// Maps topic hashes to broadcast channels that distribute incoming messages
/// for that topic to all interested subscribers.
pub type Subscriptions = HashMap<gossipsub::TopicHash, broadcast::Sender<GossipsubMessage>>;

/// Trait for network behaviours that include gossipsub functionality.
///
/// This trait provides access to the gossipsub behaviour within a composite
/// network behaviour. It's used by other traits that need to interact with
/// the gossipsub protocol.
pub trait GossipsubBehaviour {
    /// Returns a mutable reference to the gossipsub behaviour.
    fn gossipsub(&mut self) -> &mut gossipsub::Behaviour;
}

/// Actions that can be sent to a gossipsub driver for processing.
///
/// These actions represent gossipsub-related operations that need to be handled
/// by the network event loop. Each action includes the necessary data and
/// a response channel to communicate the result back to the caller.
pub enum GossipsubAction {
    /// Publish a message to the specified topic.
    ///
    /// The message data will be broadcast to all peers subscribed to the topic.
    /// The sender will receive confirmation of successful publication or an error.
    Publish(
        gossipsub::IdentTopic,
        Vec<u8>,
        oneshot::Sender<Result<(), GossipsubError>>,
    ),

    /// Subscribe to the specified topic.
    ///
    /// The sender will receive a broadcast receiver that can be used to
    /// receive messages published to this topic.
    Subscribe(
        gossipsub::IdentTopic,
        oneshot::Sender<Result<broadcast::Receiver<GossipsubMessage>, GossipsubError>>,
    ),

    /// Unsubscribe from the specified topic.
    ///
    /// After unsubscribing, no new messages for this topic will be received.
    /// The sender will receive confirmation of successful unsubscription.
    Unsubscribe(
        gossipsub::IdentTopic,
        oneshot::Sender<Result<(), GossipsubError>>,
    ),
}

/// Errors that can occur during gossipsub operations.
///
/// This enum wraps the various error types that can be returned by the
/// libp2p gossipsub implementation, providing a unified error type for
/// the Hypha network layer.
#[derive(Debug, Error)]
pub enum GossipsubError {
    /// Error occurred during subscription or unsubscription operations.
    #[error("subscription error: {0}")]
    Subscription(#[from] gossipsub::SubscriptionError),

    /// Error occurred during message publication.
    #[error("publish error: {0}")]
    Publish(#[from] gossipsub::PublishError),
}

/// Trait for handling gossipsub operations within a swarm driver.
///
/// This trait extends [`SwarmDriver`] to provide gossipsub-specific
/// functionality. Implementations should track active subscriptions and
/// process gossipsub-related events from the libp2p swarm.
pub trait GossipsubDriver<TBehavior>: SwarmDriver<TBehavior> + Send
where
    TBehavior: NetworkBehaviour + GossipsubBehaviour,
{
    /// Returns a mutable reference to the active subscriptions tracker.
    ///
    /// This is used to manage broadcast channels for distributing incoming
    /// messages to subscribers.
    fn subscriptions(&mut self) -> &mut Subscriptions;

    /// Process a gossipsub action by interacting with the gossipsub behaviour.
    ///
    /// This method handles all types of gossipsub actions including publishing,
    /// subscribing, and unsubscribing. It manages the subscription state and
    /// sends appropriate responses through the provided channels.
    ///
    /// # Arguments
    ///
    /// * `action` - The gossipsub action to process
    fn process_gossipsub_action(
        &mut self,
        action: GossipsubAction,
    ) -> impl Future<Output = ()> + Send {
        async move {
            match action {
                GossipsubAction::Publish(topic, data, tx) => {
                    if let Err(e) = self
                        .swarm()
                        .behaviour_mut()
                        .gossipsub()
                        .publish(topic, data)
                    {
                        let _ = tx.send(Err(GossipsubError::Publish(e)));
                    } else {
                        let _ = tx.send(Ok(()));
                    }
                }
                GossipsubAction::Subscribe(topic, tx) => {
                    match self.swarm().behaviour_mut().gossipsub().subscribe(&topic) {
                        Ok(true) => {
                            let (pubsub_tx, pubsub_rx) = broadcast::channel(5);
                            self.subscriptions().insert(topic.hash(), pubsub_tx);
                            let _ = tx.send(Ok(pubsub_rx));
                        }
                        Ok(false) => {
                            // We already subscribed to this topic and create a new receiver from the existing sender.
                            // We expect the sender to be present in the map, therefore we unwrap.
                            let pubsub_rx = self
                                .subscriptions()
                                .get(&topic.hash())
                                .map(|sub_tx| sub_tx.subscribe())
                                .expect("subscription should exist for already subscribed topic");
                            let _ = tx.send(Ok(pubsub_rx));
                        }
                        Err(e) => {
                            let _ = tx.send(Err(GossipsubError::Subscription(e)));
                        }
                    };
                }
                GossipsubAction::Unsubscribe(topic, tx) => {
                    if self.swarm().behaviour_mut().gossipsub().unsubscribe(&topic) {
                        self.subscriptions().remove(&topic.hash());
                    }
                    let _ = tx.send(Ok(()));
                }
            }
        }
    }

    /// Handle incoming gossipsub events from the libp2p swarm.
    ///
    /// This method processes gossipsub events, particularly incoming messages,
    /// and distributes them to the appropriate subscribers through broadcast
    /// channels.
    ///
    /// # Arguments
    ///
    /// * `event` - The gossipsub event to process
    fn process_gossipsub_event(
        &mut self,
        event: gossipsub::Event,
    ) -> impl Future<Output = ()> + Send {
        async move {
            match event {
                gossipsub::Event::Message { message, .. } => {
                    if message.source.is_none() {
                        tracing::debug!("Message from unknown source");

                        return;
                    }

                    let topic = message.topic;
                    if let Some(sub_tx) = self.subscriptions().get(&topic) {
                        let gossipsub_msg = GossipsubMessage {
                            // NOTE: We checked for `None` above, so we can safely `expect` here.
                            source: message.source.expect("source should be known"),
                            data: message.data,
                        };
                        let _ = sub_tx.send(gossipsub_msg);
                    } else {
                        tracing::debug!("No subscription for topic: {:?}", topic);
                    }
                }
                _ => {
                    tracing::debug!("Unhandled gossipsub event: {:?}", event);
                }
            }
        }
    }
}

/// Interface for sending gossipsub actions to a network driver.
///
/// This trait provides a high-level API for gossipsub operations without
/// needing to interact directly with the libp2p swarm. Implementations
/// typically send actions through a channel to the network event loop.
///
/// # Examples
///
/// ```rust,no_run
/// use hypha_network::gossipsub::{GossipsubInterface, GossipsubAction};
/// use tokio_stream::StreamExt;
///
/// async fn example_usage(network: impl GossipsubInterface) {
///     // Subscribe to a topic
///     let mut stream = network.subscribe("my-topic").await.unwrap();
///
///     // Publish a message
///     network.publish("my-topic", b"Hello, world!").await.unwrap();
///
///     // Receive the message
///     if let Some(Ok(message)) = stream.next().await {
///         println!("Received: {:?}", message);
///     }
/// }
/// ```
pub trait GossipsubInterface: Sync {
    /// Send a gossipsub action to the network driver.
    ///
    /// This is the low-level method for sending actions. Most users should
    /// prefer the higher-level methods like [`publish`](Self::publish),
    /// [`subscribe`](Self::subscribe), and [`unsubscribe`](Self::unsubscribe).
    fn send(&self, action: GossipsubAction) -> impl Future<Output = ()> + Send;

    /// Subscribe to a gossipsub topic.
    ///
    /// This method subscribes to the specified topic and returns a stream
    /// that will receive all messages published to that topic.
    ///
    /// # Arguments
    ///
    /// * `topic` - The topic name to subscribe to
    ///
    /// # Returns
    ///
    /// A stream of message data for the subscribed topic.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hypha_network::gossipsub::GossipsubInterface;
    /// # use tokio_stream::StreamExt;
    /// # async fn example(network: impl GossipsubInterface) -> Result<(), hypha_network::gossipsub::GossipsubError> {
    /// let mut stream = network.subscribe("chat").await?;
    /// while let Some(Ok(message)) = stream.next().await {
    ///     println!("Chat message: {:?}", String::from_utf8_lossy(&message.data));
    /// }
    /// # Ok(())
    /// # }
    /// ```
    fn subscribe(
        &self,
        topic: &str,
    ) -> impl Future<Output = Result<BroadcastStream<GossipsubMessage>, GossipsubError>> + Send
    {
        async move {
            let (tx, rx) = oneshot::channel();
            self.send(GossipsubAction::Subscribe(
                gossipsub::IdentTopic::new(topic),
                tx,
            ))
            .await;

            let sub_rx = rx
                .await
                .expect("driver should respond to interface requests")?;
            Ok(BroadcastStream::new(sub_rx))
        }
    }

    /// Unsubscribe from a gossipsub topic.
    ///
    /// This method unsubscribes from the specified topic. After unsubscribing,
    /// no new messages for this topic will be received.
    ///
    /// # Arguments
    ///
    /// * `topic` - The topic name to unsubscribe from
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hypha_network::gossipsub::GossipsubInterface;
    /// # async fn example(network: impl GossipsubInterface) -> Result<(), hypha_network::gossipsub::GossipsubError> {
    /// network.unsubscribe("old-topic").await?;
    /// println!("Unsubscribed from old-topic");
    /// # Ok(())
    /// # }
    /// ```
    fn unsubscribe(&self, topic: &str) -> impl Future<Output = Result<(), GossipsubError>> + Send {
        async move {
            let (tx, rx) = oneshot::channel();
            self.send(GossipsubAction::Unsubscribe(
                gossipsub::IdentTopic::new(topic),
                tx,
            ))
            .await;
            rx.await
                .expect("driver should respond to interface requests")
        }
    }

    /// Publish a message to a gossipsub topic.
    ///
    /// This method publishes the provided data to all peers subscribed to
    /// the specified topic.
    ///
    /// # Arguments
    ///
    /// * `topic` - The topic name to publish to
    /// * `data` - The data to publish (anything that can be converted to `Vec<u8>`)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hypha_network::gossipsub::GossipsubInterface;
    /// # async fn example(network: impl GossipsubInterface) -> Result<(), hypha_network::gossipsub::GossipsubError> {
    /// // Publish a string message
    /// network.publish("chat", "Hello, everyone!").await?;
    ///
    /// // Publish binary data
    /// network.publish("data", vec![1, 2, 3, 4]).await?;
    /// # Ok(())
    /// # }
    /// ```
    fn publish<T>(
        &self,
        topic: &str,
        data: T,
    ) -> impl Future<Output = Result<(), GossipsubError>> + Send
    where
        T: Into<Vec<u8>> + Send,
    {
        async move {
            let (tx, rx) = oneshot::channel();
            self.send(GossipsubAction::Publish(
                gossipsub::IdentTopic::new(topic),
                data.into(),
                tx,
            ))
            .await;
            rx.await
                .expect("driver should respond to interface requests")
        }
    }
}

#[cfg(test)]
mod tests {
    use mockall::mock;
    use tokio_stream::StreamExt;

    use super::*;

    mock! {
        TestInterface {}

        impl GossipsubInterface for TestInterface {
            async fn send(&self, action: GossipsubAction);
        }
    }

    #[tokio::test]
    async fn test_gossipsub_interface_subscribe() {
        let (sub_tx, _) = broadcast::channel(1);
        let mock_sub_tx = sub_tx.clone();
        let mut mock = MockTestInterface::new();

        mock.expect_send()
            .withf(|action| matches!(action, GossipsubAction::Subscribe(_, _)))
            .times(1)
            .returning(move |action| {
                if let GossipsubAction::Subscribe(_, tx) = action {
                    let sub_rx = mock_sub_tx.subscribe();
                    let _ = tx.send(Ok(sub_rx));
                }
            });

        let result = mock.subscribe("test_topic").await;
        assert!(result.is_ok());
        let mut result = result.unwrap();

        let test_peer_id = PeerId::random();
        sub_tx
            .send(GossipsubMessage {
                source: test_peer_id,
                data: vec![1, 2, 3],
            })
            .unwrap();

        let received = result.next().await.unwrap();
        assert_eq!(
            received,
            Ok(GossipsubMessage {
                source: test_peer_id,
                data: vec![1, 2, 3]
            })
        );
    }

    #[tokio::test]
    async fn test_gossipsub_interface_unsubscribe() {
        let mut mock = MockTestInterface::new();

        mock.expect_send()
            .withf(|action| matches!(action, GossipsubAction::Unsubscribe(_, _)))
            .times(1)
            .returning(move |action| {
                if let GossipsubAction::Unsubscribe(_, tx) = action {
                    let _ = tx.send(Ok(()));
                }
            });

        let result = mock.unsubscribe("test_topic").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_gossipsub_interface_publish() {
        let (sub_tx, mut sub_rx) = broadcast::channel(1);
        let mut mock = MockTestInterface::new();
        let test_peer_id = PeerId::random();

        mock.expect_send()
            .withf(|action| matches!(action, GossipsubAction::Publish(_, _, _)))
            .times(1)
            .returning(move |action| {
                if let GossipsubAction::Publish(_, data, tx) = action {
                    let _ = sub_tx.send(GossipsubMessage {
                        source: test_peer_id,
                        data,
                    });
                    let _ = tx.send(Ok(()));
                }
            });

        let result = mock.publish("test_topic", vec![1, 2, 3]).await;
        assert!(result.is_ok());

        let received = sub_rx.recv().await.unwrap();
        assert_eq!(
            received,
            GossipsubMessage {
                source: test_peer_id,
                data: vec![1, 2, 3]
            }
        );
    }
}
