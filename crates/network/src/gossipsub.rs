use std::{collections::HashSet, error::Error, fmt::Display};

use libp2p::{gossipsub, swarm::NetworkBehaviour};
use tokio::sync::oneshot;

use crate::swarm::SwarmDriver;

pub type Subscriptions = HashSet<gossipsub::TopicHash>;

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
        oneshot::Sender<Result<(), GossipsubError>>,
    ),
    Unsubscribe(
        gossipsub::IdentTopic,
        oneshot::Sender<Result<(), GossipsubError>>,
    ),
}

pub enum GossipsubEvent {
    Message(gossipsub::TopicHash, Vec<u8>),
}

#[derive(Debug)]
pub enum GossipsubError {
    Subscription(gossipsub::SubscriptionError),
    Publish(gossipsub::PublishError),
}

impl Error for GossipsubError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

impl Display for GossipsubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Gossipsub error")
    }
}

#[allow(async_fn_in_trait)]
pub trait GossipsubDriver<TBehavior>: SwarmDriver<TBehavior>
where
    TBehavior: NetworkBehaviour + GossipsubBehaviour,
{
    async fn send(&mut self, event: GossipsubEvent);

    fn subscriptions(&mut self) -> &mut Subscriptions;

    async fn process_gossipsub_action(&mut self, action: GossipsubAction) {
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
                        self.subscriptions().insert(topic.hash());
                        let _ = tx.send(Ok(()));
                    }
                    Ok(false) => {
                        // We already subscribed to this topic and create a new receiver from the existing sender.
                        // We expect the sender to be present in the map, therefore we unwrap.
                        let _ = self.subscriptions().get(&topic.hash()).unwrap();
                        let _ = tx.send(Ok(()));
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

    async fn process_gossipsub_event(&mut self, event: gossipsub::Event) {
        match event {
            gossipsub::Event::Message { message, .. } => {
                tracing::debug!("Received gossipsub message: {:?}", message);
                let topic = message.topic;
                if self.subscriptions().get(&topic).is_some() {
                    self.send(GossipsubEvent::Message(topic, message.data))
                        .await;
                } else {
                    tracing::warn!("No subscription for topic: {:?}", topic);
                }
            }
            _ => {
                tracing::debug!("Unhandled gossipsub event: {:?}", event);
            }
        }
    }
}

#[allow(async_fn_in_trait)]
pub trait GossipsubInterface {
    async fn send(&self, action: GossipsubAction);

    async fn subscribe(&self, topic: &str) -> Result<(), GossipsubError> {
        let (tx, rx) = oneshot::channel();
        self.send(GossipsubAction::Subscribe(
            gossipsub::IdentTopic::new(topic),
            tx,
        ))
        .await;
        rx.await.unwrap()
    }

    async fn unsubscribe(&self, topic: &str) -> Result<(), GossipsubError> {
        let (tx, rx) = oneshot::channel();
        self.send(GossipsubAction::Unsubscribe(
            gossipsub::IdentTopic::new(topic),
            tx,
        ))
        .await;
        rx.await.unwrap()
    }

    async fn publish<T>(&self, topic: &str, data: T) -> Result<(), GossipsubError>
    where
        T: Into<Vec<u8>>,
    {
        let (tx, rx) = oneshot::channel();
        self.send(GossipsubAction::Publish(
            gossipsub::IdentTopic::new(topic),
            data.into(),
            tx,
        ))
        .await;
        rx.await.unwrap()
    }
}

#[cfg(test)]
mod tests {
    use mockall::mock;

    use super::*;

    mock! {
        TestInterface {}

        impl GossipsubInterface for TestInterface {
            async fn send(&self, action: GossipsubAction);
        }
    }

    #[tokio::test]
    async fn test_gossipsub_interface_subscribe() {
        let mut mock = MockTestInterface::new();

        mock.expect_send()
            .withf(|action| matches!(action, GossipsubAction::Subscribe(_, _)))
            .times(1)
            .returning(|action| match action {
                GossipsubAction::Subscribe(_, tx) => {
                    let _ = tx.send(Ok(()));
                }
                _ => {}
            });

        let result = mock.subscribe("test_topic").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_gossipsub_interface_unsubscribe() {
        let mut mock = MockTestInterface::new();

        mock.expect_send()
            .withf(|action| matches!(action, GossipsubAction::Unsubscribe(_, _)))
            .times(1)
            .returning(|action| match action {
                GossipsubAction::Unsubscribe(_, tx) => {
                    let _ = tx.send(Ok(()));
                }
                _ => {}
            });

        let result = mock.unsubscribe("test_topic").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_gossipsub_interface_publish() {
        let mut mock = MockTestInterface::new();

        mock.expect_send()
            .withf(|action| matches!(action, GossipsubAction::Publish(_, _, _)))
            .times(1)
            .returning(|action| match action {
                GossipsubAction::Publish(_, data, tx) => {
                    if data == vec![1, 2, 3] {
                        let _ = tx.send(Ok(()));
                    }
                }
                _ => {}
            });

        let result = mock.publish("test_topic", vec![1, 2, 3]).await;
        assert!(result.is_ok());
    }
}
