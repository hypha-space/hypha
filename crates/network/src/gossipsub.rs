use std::collections::HashMap;

use libp2p::{gossipsub, swarm::NetworkBehaviour};
use thiserror::Error;
use tokio::sync::{broadcast, oneshot};
use tokio_stream::wrappers::BroadcastStream;

use crate::swarm::SwarmDriver;

pub type Subscriptions = HashMap<gossipsub::TopicHash, broadcast::Sender<Vec<u8>>>;

pub trait GossipsubBehaviour {
    fn gossipsub(&mut self) -> &mut gossipsub::Behaviour;
}

pub enum GossipsubAction {
    Publish(
        gossipsub::IdentTopic,
        Vec<u8>,
        oneshot::Sender<Result<(), GossipsubError>>,
    ),
    Subscribe(
        gossipsub::IdentTopic,
        oneshot::Sender<Result<broadcast::Receiver<Vec<u8>>, GossipsubError>>,
    ),
    Unsubscribe(
        gossipsub::IdentTopic,
        oneshot::Sender<Result<(), GossipsubError>>,
    ),
}

#[derive(Debug, Error)]
pub enum GossipsubError {
    #[error("subscription error: {0}")]
    Subscription(#[from] gossipsub::SubscriptionError),
    #[error("publish error: {0}")]
    Publish(#[from] gossipsub::PublishError),
}

pub trait GossipsubDriver<TBehavior>: SwarmDriver<TBehavior> + Send
where
    TBehavior: NetworkBehaviour + GossipsubBehaviour,
{
    fn subscriptions(&mut self) -> &mut Subscriptions;

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

    fn process_gossipsub_event(
        &mut self,
        event: gossipsub::Event,
    ) -> impl Future<Output = ()> + Send {
        async move {
            match event {
                gossipsub::Event::Message { message, .. } => {
                    let topic = message.topic;
                    if let Some(sub_tx) = self.subscriptions().get(&topic) {
                        let _ = sub_tx.send(message.data);
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

pub trait GossipsubInterface: Sync {
    fn send(&self, action: GossipsubAction) -> impl Future<Output = ()> + Send;

    fn subscribe(
        &self,
        topic: &str,
    ) -> impl Future<Output = Result<BroadcastStream<Vec<u8>>, GossipsubError>> + Send {
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

        sub_tx.send(vec![1, 2, 3]).unwrap();

        let received = result.next().await.unwrap();
        assert_eq!(received, Ok(vec![1, 2, 3]));
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

        mock.expect_send()
            .withf(|action| matches!(action, GossipsubAction::Publish(_, _, _)))
            .times(1)
            .returning(move |action| {
                if let GossipsubAction::Publish(_, data, tx) = action {
                    let _ = sub_tx.send(data);
                    let _ = tx.send(Ok(()));
                }
            });

        let result = mock.publish("test_topic", vec![1, 2, 3]).await;
        assert!(result.is_ok());

        let received = sub_rx.recv().await.unwrap();
        assert_eq!(received, vec![1, 2, 3]);
    }
}
