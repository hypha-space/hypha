use std::{collections::HashMap, time::Duration};

use futures::StreamExt;
use hypha_network::{gossipsub::*, swarm::SwarmDriver};
use libp2p::{
    Swarm, gossipsub, identify,
    kad::{self, store::MemoryStore},
    swarm::{NetworkBehaviour, SwarmEvent},
};
use libp2p_swarm_test::SwarmExt;
use tokio::sync::mpsc;

#[derive(NetworkBehaviour)]
struct TestBehaviour {
    gossipsub: gossipsub::Behaviour,
    identify: identify::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
}

impl GossipsubBehaviour for TestBehaviour {
    fn gossipsub(&mut self) -> &mut gossipsub::Behaviour {
        &mut self.gossipsub
    }
}

struct TestDriver {
    swarm: Swarm<TestBehaviour>,
    subscriptions: Subscriptions,
    action_rx: mpsc::UnboundedReceiver<GossipsubAction>,
}

impl SwarmDriver<TestBehaviour> for TestDriver {
    fn swarm(&mut self) -> &mut Swarm<TestBehaviour> {
        &mut self.swarm
    }

    async fn run(mut self) -> Result<(), hypha_network::swarm::SwarmError> {
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(TestBehaviourEvent::Gossipsub(event)) => {
                            self.process_gossipsub_event(event).await;
                        }
                        SwarmEvent::Behaviour(TestBehaviourEvent::Identify(event)) => {
                            if let identify::Event::Received { peer_id, info, .. } = event {
                                for addr in info.listen_addrs {
                                    self.swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Some(action) = self.action_rx.recv() => {
                    self.process_gossipsub_action(action).await;
                }
                else => break
            }
        }

        Ok(())
    }
}

impl GossipsubDriver<TestBehaviour> for TestDriver {
    fn subscriptions(&mut self) -> &mut Subscriptions {
        &mut self.subscriptions
    }
}

#[derive(Clone)]
struct TestInterface {
    action_tx: mpsc::UnboundedSender<GossipsubAction>,
}

impl TestInterface {
    fn create(swarm: Swarm<TestBehaviour>) -> (Self, TestDriver) {
        let (action_tx, action_rx) = mpsc::unbounded_channel();

        let driver = TestDriver {
            swarm,
            subscriptions: HashMap::new(),
            action_rx,
        };

        let interface = Self { action_tx };

        (interface, driver)
    }
}

impl GossipsubInterface for TestInterface {
    async fn send(&self, action: GossipsubAction) {
        self.action_tx.send(action).expect("Driver dropped");
    }
}

fn create_test_swarm() -> Swarm<TestBehaviour> {
    Swarm::new_ephemeral_tokio(|key| {
        let peer_id = key.public().to_peer_id();
        TestBehaviour {
            gossipsub: gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub::Config::default(),
            )
            .expect("Valid configuration"),
            identify: identify::Behaviour::new(identify::Config::new(
                "/test-identify/0.1.0".to_string(),
                key.public(),
            )),
            kademlia: kad::Behaviour::new(peer_id, MemoryStore::new(peer_id)),
        }
    })
}

#[tokio::test]
async fn single_subscriber_receives_published_message() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    swarm1.connect(&mut swarm2).await;

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let mut subscription = interface2.subscribe("test_topic").await.unwrap();

    interface1
        .publish("test_topic", b"Hello World")
        .await
        .unwrap();

    let received = tokio::time::timeout(Duration::from_secs(5), subscription.next())
        .await
        .expect("Should receive message")
        .expect("Should have message")
        .expect("Should be Ok");

    assert_eq!(received.data, b"Hello World");
}

#[tokio::test]
async fn multiple_subscribers_all_receive_message() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();
    let mut swarm3 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;
    swarm3.listen().with_memory_addr_external().await;

    swarm1.connect(&mut swarm2).await;
    swarm1.connect(&mut swarm3).await;

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);
    let (interface3, driver3) = TestInterface::create(swarm3);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());
    tokio::spawn(driver3.run());

    let mut subscription2 = interface2.subscribe("broadcast_topic").await.unwrap();
    let mut subscription3 = interface3.subscribe("broadcast_topic").await.unwrap();

    interface1
        .publish("broadcast_topic", b"Broadcast Message")
        .await
        .unwrap();

    let received2 = tokio::time::timeout(Duration::from_secs(5), subscription2.next())
        .await
        .expect("Peer 2 should receive message")
        .expect("Should have message")
        .expect("Should be Ok");

    let received3 = tokio::time::timeout(Duration::from_secs(5), subscription3.next())
        .await
        .expect("Peer 3 should receive message")
        .expect("Should have message")
        .expect("Should be Ok");

    assert_eq!(received2.data, b"Broadcast Message");
    assert_eq!(received3.data, b"Broadcast Message");
}

#[tokio::test]
async fn multiple_topics_deliver_to_correct_subscribers() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    swarm1.connect(&mut swarm2).await;

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let mut topic_a_sub = interface2.subscribe("topic_a").await.unwrap();
    let mut topic_b_sub = interface2.subscribe("topic_b").await.unwrap();

    interface1.publish("topic_a", b"Message A").await.unwrap();
    interface1.publish("topic_b", b"Message B").await.unwrap();

    let received_a = tokio::time::timeout(Duration::from_secs(5), topic_a_sub.next())
        .await
        .expect("Should receive topic A message")
        .expect("Should have message")
        .expect("Should be Ok");

    let received_b = tokio::time::timeout(Duration::from_secs(5), topic_b_sub.next())
        .await
        .expect("Should receive topic B message")
        .expect("Should have message")
        .expect("Should be Ok");

    assert_eq!(received_a.data, b"Message A");
    assert_eq!(received_b.data, b"Message B");
}

#[tokio::test]
async fn duplicate_subscriptions_both_receive_message() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    swarm1.connect(&mut swarm2).await;

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let mut subscription1 = interface2.subscribe("duplicate_topic").await.unwrap();
    let mut subscription2 = interface2.subscribe("duplicate_topic").await.unwrap();

    interface1
        .publish("duplicate_topic", b"Duplicate Test")
        .await
        .unwrap();

    let received1 = tokio::time::timeout(Duration::from_secs(5), subscription1.next())
        .await
        .expect("First subscription should receive message")
        .expect("Should have message")
        .expect("Should be Ok");

    let received2 = tokio::time::timeout(Duration::from_secs(5), subscription2.next())
        .await
        .expect("Second subscription should receive message")
        .expect("Should have message")
        .expect("Should be Ok");

    assert_eq!(received1.data, b"Duplicate Test");
    assert_eq!(received2.data, b"Duplicate Test");
}

#[tokio::test]
async fn no_subscribers_publish_fails() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    swarm1.connect(&mut swarm2).await;

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (_, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let result = interface1.publish("test_topic", b"Test message").await;

    assert!(
        matches!(result, Err(GossipsubError::Publish(_))),
        "Publish fails if there are no subscribers"
    );
}

#[tokio::test]
async fn isolated_peer_publish_fails() {
    let swarm = create_test_swarm();

    let (interface, driver) = TestInterface::create(swarm);
    tokio::spawn(driver.run());

    let result = interface.publish("test_topic", b"Test message").await;
    assert!(matches!(result, Err(GossipsubError::Publish(_))));
}
