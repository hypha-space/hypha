use std::{collections::HashMap, time::Duration};

use futures_util::StreamExt;
use hypha_network::{
    IpNet,
    dial::*,
    kad::*,
    swarm::{SwarmDriver, SwarmError},
};
use libp2p::{
    PeerId, Swarm, identify,
    kad::{self, store::MemoryStore},
    swarm::{NetworkBehaviour, SwarmEvent},
};
use libp2p_swarm_test::SwarmExt;
use tokio::{sync::mpsc, time::timeout};

#[derive(NetworkBehaviour)]
struct TestBehaviour {
    kademlia: kad::Behaviour<MemoryStore>,
    identify: identify::Behaviour,
}

impl KademliaBehavior for TestBehaviour {
    fn kademlia(&mut self) -> &mut kad::Behaviour<MemoryStore> {
        &mut self.kademlia
    }
}

struct TestDriver {
    swarm: Swarm<TestBehaviour>,
    pending_queries: PendingQueries,
    pending_dials: PendingDials,
    action_rx: mpsc::UnboundedReceiver<Action>,
    pending_bootstrap: std::sync::Arc<tokio::sync::SetOnce<()>>,
}

enum Action {
    Kademlia(KademliaAction),
    Dial(DialAction),
}

impl SwarmDriver<TestBehaviour> for TestDriver {
    fn swarm(&mut self) -> &mut Swarm<TestBehaviour> {
        &mut self.swarm
    }

    async fn run(mut self) -> Result<(), SwarmError> {
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::ConnectionEstablished { connection_id, peer_id, .. } => {
                            self.process_connection_established(peer_id, &connection_id).await;
                            // Trigger bootstrap once connected to at least one peer
                            let _ = self.swarm.behaviour_mut().kademlia.bootstrap();
                        }
                        SwarmEvent::OutgoingConnectionError { connection_id, error, .. } => {
                            self.process_connection_error(&connection_id, error).await;
                        }
                        SwarmEvent::Behaviour(TestBehaviourEvent::Identify(event)) => {
                            self.process_identify_event(event);
                        }
                        SwarmEvent::Behaviour(TestBehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed {
                            id,
                            result,
                            step,
                            ..
                        })) => {
                            self.process_kademlia_query_result(id, result, step).await;
                        }
                        _ => {}
                    }
                }
                Some(action) = self.action_rx.recv() => {
                    match action {
                        Action::Kademlia(action) => {
                            self.process_kademlia_action(action).await;
                        }
                        Action::Dial(action) => {
                            self.process_dial_action(action).await;
                        }
                    }
                }
                else => break
            }
        }

        Ok(())
    }
}

impl KademliaDriver<TestBehaviour> for TestDriver {
    fn pending_queries(&mut self) -> &mut PendingQueries {
        &mut self.pending_queries
    }

    fn pending_bootstrap(&mut self) -> &mut std::sync::Arc<tokio::sync::SetOnce<()>> {
        &mut self.pending_bootstrap
    }

    fn exclude_cidrs(&self) -> Vec<IpNet> {
        Vec::new()
    }
}

impl DialDriver<TestBehaviour> for TestDriver {
    fn pending_dials(&mut self) -> &mut PendingDials {
        &mut self.pending_dials
    }

    fn exclude_cidrs(&self) -> &[IpNet] {
        &[]
    }
}

#[derive(Clone)]
struct TestInterface {
    action_tx: mpsc::UnboundedSender<Action>,
}

impl TestInterface {
    fn create(swarm: Swarm<TestBehaviour>) -> (Self, TestDriver) {
        let (action_tx, action_rx) = mpsc::unbounded_channel();

        let driver = TestDriver {
            swarm,
            pending_queries: HashMap::new(),
            pending_dials: HashMap::new(),
            action_rx,
            pending_bootstrap: std::sync::Arc::new(tokio::sync::SetOnce::new()),
        };

        let interface = Self { action_tx };

        (interface, driver)
    }
}

impl KademliaInterface for TestInterface {
    async fn send(&self, action: KademliaAction) {
        self.action_tx
            .send(Action::Kademlia(action))
            .expect("Driver dropped");
    }
}

impl DialInterface for TestInterface {
    async fn send(&self, action: DialAction) {
        self.action_tx
            .send(Action::Dial(action))
            .expect("Driver dropped");
    }
}

fn create_test_swarm() -> Swarm<TestBehaviour> {
    Swarm::new_ephemeral_tokio(|key| {
        let peer_id = key.public().to_peer_id();
        TestBehaviour {
            kademlia: kad::Behaviour::new(peer_id, MemoryStore::new(peer_id)),
            identify: identify::Behaviour::new(identify::Config::new(
                "/test-identify/0.1.0".to_string(),
                key.public(),
            )),
        }
    })
}

#[tokio::test]
async fn test_store_and_get_record() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    let swarm2_addr = swarm2.external_addresses().next().unwrap().clone();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let _ = interface1
        .dial(swarm2_addr)
        .await
        .expect("Dial should succeed");

    interface1
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface1");
    interface2
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface2");

    let record = kad::Record::new(kad::RecordKey::new(&"test_key"), b"test_value".to_vec());
    let store_result = interface1.store(record.clone()).await;

    assert!(
        store_result.is_ok(),
        "Store should succeed: {:?}",
        store_result
    );

    let get_result = interface2.get("test_key").await.unwrap();

    assert_eq!(get_result.key, record.key);
    assert_eq!(get_result.value, record.value);
}

#[tokio::test]
async fn test_provide_and_find_providers() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    let peer1_id = *swarm1.local_peer_id();
    let swarm2_addr = swarm2.external_addresses().next().unwrap().clone();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let _ = interface1
        .dial(swarm2_addr)
        .await
        .expect("Dial should succeed");

    interface1
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface1");
    interface2
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface2");

    let provide_result = interface1.provide("test").await;

    assert!(
        provide_result.is_ok(),
        "Provide should succeed: {:?}",
        provide_result
    );

    let find_result = interface2.find_provider("test").await.unwrap();

    assert!(
        find_result.contains(&peer1_id),
        "Should contain the providing peer"
    );
}

#[tokio::test]
async fn test_no_provider_found() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    let swarm2_addr = swarm2.external_addresses().next().unwrap().clone();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let _ = interface1
        .dial(swarm2_addr)
        .await
        .expect("Dial should succeed");

    interface1
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface1");
    interface2
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface2");

    let find_result = interface2.find_provider("non-existent-key").await.unwrap();

    assert!(
        find_result.is_empty(),
        "Should not find providers for a non-existent key"
    );
}

#[tokio::test]
async fn test_multiple_providers() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();
    let mut swarm3 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;
    swarm3.listen().with_memory_addr_external().await;

    let peer1_id = *swarm1.local_peer_id();
    let peer2_id = *swarm2.local_peer_id();
    let swarm2_addr = swarm2.external_addresses().next().unwrap().clone();
    let swarm3_addr = swarm3.external_addresses().next().unwrap().clone();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);
    let (interface3, driver3) = TestInterface::create(swarm3);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());
    tokio::spawn(driver3.run());

    let _ = interface1
        .dial(swarm2_addr)
        .await
        .expect("Dial should succeed");
    let _ = interface1
        .dial(swarm3_addr)
        .await
        .expect("Dial should succeed");

    interface1
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface1");
    interface2
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface2");
    interface3
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface3");

    interface1
        .provide("shared-key")
        .await
        .expect("Provide should succeed");
    interface2
        .provide("shared-key")
        .await
        .expect("Provide should succeed");

    let find_result = interface3.find_provider("shared-key").await.unwrap();

    assert_eq!(
        find_result.len(),
        2,
        "Should find two providers for the shared key"
    );
    assert!(
        find_result.contains(&peer1_id),
        "Should contain peer1 as provider"
    );
    assert!(
        find_result.contains(&peer2_id),
        "Should contain peer2 as provider"
    );
}

#[tokio::test]
async fn test_empty_dht_get_closest_peers() {
    let swarm = create_test_swarm();

    let (interface, driver) = TestInterface::create(swarm);
    tokio::spawn(driver.run());

    let target_peer = PeerId::random();
    let result = interface.get_closest_peers(target_peer).await.unwrap();

    assert!(
        result.is_empty(),
        "Should return an empty list when DHT is empty {:?}",
        result
    );
}

#[tokio::test]
async fn test_get_closest_peers() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    let peer2_id = *swarm2.local_peer_id();
    let swarm2_addr = swarm2.external_addresses().next().unwrap().clone();
    let target_peer = PeerId::random();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (_, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let _ = interface1
        .dial(swarm2_addr)
        .await
        .expect("Dial should succeed");

    interface1
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface1");

    let result = interface1.get_closest_peers(target_peer).await.unwrap();

    assert!(!result.is_empty(), "Should find at least one peer");
    assert!(
        result.iter().any(|p| p.peer_id == peer2_id),
        "Should contain the connected peer"
    );
}

#[tokio::test]
async fn test_get_non_existent_record() {
    let mut swarm1 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;

    let (interface1, driver1) = TestInterface::create(swarm1);

    tokio::spawn(driver1.run());

    let get_result = interface1.get("non_existent_key").await;

    assert!(
        matches!(
            get_result,
            Err(KademliaError::GetRecord(
                kad::GetRecordError::NotFound { .. }
            ))
        ),
        "Getting non-existent key should return `NotFound`",
    );
}

#[tokio::test]
async fn test_find_providers_for_non_existent_key() {
    let mut swarm1 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;

    let (interface1, driver1) = TestInterface::create(swarm1);

    tokio::spawn(driver1.run());

    let find_result = interface1.find_provider("non_existent").await.unwrap();

    assert!(
        find_result.is_empty(),
        "Should not find providers for non-existent content"
    );
}

#[tokio::test]
async fn test_concurrent_record_operations() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    let swarm2_addr = swarm2.external_addresses().next().unwrap().clone();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let _ = interface1
        .dial(swarm2_addr)
        .await
        .expect("Dial should succeed");

    interface1
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface1");
    interface2
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface2");

    let record1 = kad::Record::new(kad::RecordKey::new(&"concurrent_key1"), b"value1".to_vec());
    let record2 = kad::Record::new(kad::RecordKey::new(&"concurrent_key2"), b"value2".to_vec());
    let record3 = kad::Record::new(kad::RecordKey::new(&"concurrent_key3"), b"value3".to_vec());

    let store_task1 = interface1.store(record1.clone());
    let store_task2 = interface1.store(record2.clone());
    let store_task3 = interface1.store(record3.clone());

    let (result1, result2, result3) = tokio::join!(store_task1, store_task2, store_task3);

    assert!(result1.is_ok(), "Concurrent store 1 should succeed");
    assert!(result2.is_ok(), "Concurrent store 2 should succeed");
    assert!(result3.is_ok(), "Concurrent store 3 should succeed");

    let get_task1 = interface2.get("concurrent_key1");
    let get_task2 = interface2.get("concurrent_key2");
    let get_task3 = interface2.get("concurrent_key3");

    let (get_result1, get_result2, get_result3) = tokio::join!(get_task1, get_task2, get_task3);

    assert!(get_result1.is_ok(), "Concurrent get 1 should succeed");
    assert!(get_result2.is_ok(), "Concurrent get 2 should succeed");
    assert!(get_result3.is_ok(), "Concurrent get 3 should succeed");

    assert_eq!(get_result1.unwrap().value, b"value1");
    assert_eq!(get_result2.unwrap().value, b"value2");
    assert_eq!(get_result3.unwrap().value, b"value3");
}

#[tokio::test]
async fn test_record_update_and_overwrite() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    let swarm2_addr = swarm2.external_addresses().next().unwrap().clone();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let _ = interface1
        .dial(swarm2_addr)
        .await
        .expect("Dial should succeed");

    interface1
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface1");
    interface2
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete on interface2");

    let key = "updateable_key";

    let initial_record = kad::Record::new(kad::RecordKey::new(&key), b"initial_value".to_vec());
    let store_result = interface1.store(initial_record).await;
    assert!(store_result.is_ok(), "Initial store should succeed ");

    let get_result = interface2.get(key).await.unwrap();
    assert_eq!(get_result.value, b"initial_value");

    let updated_record = kad::Record::new(kad::RecordKey::new(&key), b"updated_value".to_vec());
    let update_result = interface1.store(updated_record).await;
    assert!(update_result.is_ok(), "Update should succeed ");

    let get_updated_result = interface2.get(key).await.unwrap();
    assert_eq!(
        get_updated_result.value, b"updated_value",
        "Record should be updated "
    );

    let peer2_record = kad::Record::new(kad::RecordKey::new(&key), b"peer2_value".to_vec());
    let peer2_store_result = interface2.store(peer2_record).await;
    assert!(peer2_store_result.is_ok(), "Peer2 store should succeed");

    let final_get_result = interface1.get(key).await.unwrap();
    assert!(
        final_get_result.value == b"updated_value" || final_get_result.value == b"peer2_value",
        "Should get either updated_value or peer2_value, got: {:?}",
        String::from_utf8_lossy(&final_get_result.value)
    );
}

#[tokio::test]
async fn test_isolated_peer_operations() {
    let mut swarm1 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;

    let (interface1, driver1) = TestInterface::create(swarm1);

    tokio::spawn(driver1.run());

    let record = kad::Record::new(
        kad::RecordKey::new(&"isolated_key"),
        b"isolated_value".to_vec(),
    );

    let store_result = interface1.store(record).await;
    assert!(
        matches!(
            store_result,
            Err(KademliaError::PutRecord(
                kad::PutRecordError::QuorumFailed { .. }
            ))
        ),
        "Should fail with QuorumFailed"
    );

    let get_result = interface1.get("non_existent_isolated_key").await;
    assert!(
        matches!(
            get_result,
            Err(KademliaError::GetRecord(
                kad::GetRecordError::NotFound { .. }
            ))
        ),
        "Should fail with NotFound",
    );

    let provide_result = interface1.provide("isolated_service").await;
    assert!(provide_result.is_ok());

    let find_result = interface1
        .find_provider("non_existent_service")
        .await
        .unwrap();
    assert!(find_result.is_empty(), "Should return emoty set");

    let target_id = PeerId::random();
    let closest_result = interface1.get_closest_peers(target_id).await.unwrap();
    assert!(
        closest_result.is_empty(),
        "Should not find other peers when isolated"
    );
}

#[tokio::test]
async fn test_wait_for_bootstrap_after_connect() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    let swarm2_addr = swarm2.external_addresses().next().unwrap().clone();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (_interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let _ = interface1
        .dial(swarm2_addr)
        .await
        .expect("Dial should succeed");

    let res = timeout(Duration::from_secs(1), interface1.wait_for_bootstrap()).await;
    assert!(res.is_ok(), "bootstrap wait timed out");
    assert!(res.unwrap().is_ok(), "bootstrap signaling failed");
}

#[tokio::test]
async fn test_wait_for_bootstrap_returns_immediately_after_complete() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    let swarm2_addr = swarm2.external_addresses().next().unwrap().clone();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (_interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let _ = interface1
        .dial(swarm2_addr)
        .await
        .expect("Dial should succeed");

    interface1
        .wait_for_bootstrap()
        .await
        .expect("bootstrap should complete");

    let res = timeout(Duration::from_micros(1), interface1.wait_for_bootstrap()).await;
    assert!(
        res.is_ok(),
        "second bootstrap wait did not return _immediately_"
    );
    assert!(res.unwrap().is_ok());
}
