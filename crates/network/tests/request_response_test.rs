use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures::StreamExt;
use hypha_network::{error::HyphaError, request_response::*, swarm::SwarmDriver};
use libp2p::{
    Swarm,
    request_response::{self, ProtocolSupport},
    swarm::{NetworkBehaviour, SwarmEvent},
};
use libp2p_swarm_test::SwarmExt;
use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, time::sleep};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
enum TestRequest {
    Ping(String),
    Echo(String),
    Work { id: u32, data: String },
    Slow { delay_ms: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
enum TestResponse {
    Pong(String),
    Echo(String),
    WorkDone { id: u32 },
    SlowDone,
}

type TestCodec = hypha_network::cbor_codec::Codec<TestRequest, TestResponse>;

#[derive(NetworkBehaviour)]
struct TestBehaviour {
    request_response: request_response::Behaviour<TestCodec>,
}

impl RequestResponseBehaviour<TestCodec> for TestBehaviour {
    fn request_response(&mut self) -> &mut request_response::Behaviour<TestCodec> {
        &mut self.request_response
    }
}

struct TestDriver {
    swarm: Swarm<TestBehaviour>,
    outbound_requests: OutboundRequests<TestCodec>,
    outbound_responses: OutboundResponses,
    request_handlers: Vec<RequestHandler<TestCodec>>,
    action_rx: mpsc::UnboundedReceiver<RequestResponseAction<TestCodec>>,
}

impl SwarmDriver<TestBehaviour> for TestDriver {
    fn swarm(&mut self) -> &mut Swarm<TestBehaviour> {
        &mut self.swarm
    }

    async fn run(mut self) -> Result<(), HyphaError> {
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => {
                    if let SwarmEvent::Behaviour(TestBehaviourEvent::RequestResponse(event)) = event {
                        self.process_request_response_event(event).await;
                    }
                }
                Some(action) = self.action_rx.recv() => {
                    self.process_request_response_action(action).await;
                }
                else => break
            }
        }

        Ok(())
    }
}

impl RequestResponseDriver<TestBehaviour, TestCodec> for TestDriver {
    fn outbound_requests(&mut self) -> &mut OutboundRequests<TestCodec> {
        &mut self.outbound_requests
    }

    fn outbound_responses(&mut self) -> &mut OutboundResponses {
        &mut self.outbound_responses
    }

    fn request_handlers(&mut self) -> &mut Vec<RequestHandler<TestCodec>> {
        &mut self.request_handlers
    }
}

#[derive(Clone)]
struct TestInterface {
    action_tx: mpsc::UnboundedSender<RequestResponseAction<TestCodec>>,
}

impl TestInterface {
    fn create(swarm: Swarm<TestBehaviour>) -> (Self, TestDriver) {
        #![allow(clippy::disallowed_methods)]
        let (action_tx, action_rx) = mpsc::unbounded_channel();

        let driver = TestDriver {
            swarm,
            outbound_requests: HashMap::new(),
            outbound_responses: HashMap::new(),
            request_handlers: Vec::new(),
            action_rx,
        };

        let interface = Self { action_tx };

        (interface, driver)
    }
}

impl RequestResponseInterface<TestCodec> for TestInterface {
    async fn send(&self, action: RequestResponseAction<TestCodec>) {
        self.action_tx.send(action).expect("Driver dropped");
    }

    fn try_send(
        &self,
        action: RequestResponseAction<TestCodec>,
    ) -> Result<(), RequestResponseError> {
        self.action_tx
            .send(action)
            .map_err(|_| RequestResponseError::Other("Driver dropped".to_string()))
    }
}

fn create_test_swarm() -> Swarm<TestBehaviour> {
    Swarm::new_ephemeral_tokio(|_| TestBehaviour {
        request_response: request_response::Behaviour::new(
            [(
                libp2p::StreamProtocol::new("/test/0.0.1"),
                ProtocolSupport::Full,
            )],
            request_response::Config::default(),
        ),
    })
}

#[tokio::test]
async fn test_simple_request_response() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;

    swarm1.connect(&mut swarm2).await;

    let peer2 = *swarm2.local_peer_id();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let handler = interface2
        .on(|req: &TestRequest| matches!(req, TestRequest::Ping(_)))
        .into_stream()
        .await
        .unwrap();

    tokio::spawn(async move {
        handler
            .respond_with_concurrent(Some(1), |(_, req)| async move {
                match req {
                    TestRequest::Ping(msg) => TestResponse::Pong(format!("Got: {msg}")),
                    _ => unreachable!(),
                }
            })
            .await;
    });

    let response = interface1
        .request(peer2, TestRequest::Ping("Hello".to_string()))
        .await
        .unwrap();

    assert_eq!(response, TestResponse::Pong("Got: Hello".to_string()));
}

#[tokio::test]
async fn test_multiple_handlers_with_patterns() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;
    swarm1.connect(&mut swarm2).await;

    let peer2 = *swarm2.local_peer_id();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let ping_handler = interface2
        .on(|req: &TestRequest| matches!(req, TestRequest::Ping(_)))
        .buffer_size(64)
        .into_stream()
        .await
        .unwrap();

    tokio::spawn(async move {
        ping_handler
            .respond_with_concurrent(Some(2), |(_, req)| async move {
                match req {
                    TestRequest::Ping(msg) => TestResponse::Pong(msg),
                    _ => unreachable!(),
                }
            })
            .await;
    });

    let echo_handler = interface2
        .on(|req: &TestRequest| matches!(req, TestRequest::Echo(_)))
        .into_stream()
        .await
        .unwrap();

    tokio::spawn(async move {
        echo_handler
            .respond_with_concurrent(Some(1), |(_, req)| async move {
                match req {
                    TestRequest::Echo(msg) => TestResponse::Echo(msg),
                    _ => unreachable!(),
                }
            })
            .await;
    });

    let ping_response = interface1
        .request(peer2, TestRequest::Ping("ping".to_string()))
        .await
        .unwrap();
    assert_eq!(ping_response, TestResponse::Pong("ping".to_string()));

    let echo_response = interface1
        .request(peer2, TestRequest::Echo("echo".to_string()))
        .await
        .unwrap();

    assert_eq!(echo_response, TestResponse::Echo("echo".to_string()));
}

#[tokio::test]
async fn test_concurrent_request_processing() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;
    swarm1.connect(&mut swarm2).await;

    let peer2 = *swarm2.local_peer_id();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let concurrent_count = Arc::new(AtomicUsize::new(0));
    let max_concurrent_seen = Arc::new(AtomicUsize::new(0));

    let concurrent_count_clone = concurrent_count.clone();
    let max_concurrent_seen_clone = max_concurrent_seen.clone();

    let handler = interface2
        .on(|req: &TestRequest| matches!(req, TestRequest::Slow { .. }))
        .into_stream()
        .await
        .unwrap();

    tokio::spawn(async move {
        handler
            .respond_with_concurrent(Some(3), move |(_, req)| {
                let concurrent_count = concurrent_count_clone.clone();
                let max_concurrent_seen = max_concurrent_seen_clone.clone();

                async move {
                    // NOTE: Track concurrent executions and update (non-blocking) max seen
                    let current = concurrent_count.fetch_add(1, Ordering::SeqCst) + 1;
                    let mut max_seen = max_concurrent_seen.load(Ordering::SeqCst);
                    while current > max_seen {
                        match max_concurrent_seen.compare_exchange_weak(
                            max_seen,
                            current,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        ) {
                            Ok(_) => break,
                            Err(x) => max_seen = x,
                        }
                    }

                    match req {
                        TestRequest::Slow { delay_ms } => {
                            sleep(Duration::from_millis(delay_ms)).await;
                            concurrent_count.fetch_sub(1, Ordering::SeqCst);
                            TestResponse::SlowDone
                        }
                        _ => unreachable!(),
                    }
                }
            })
            .await;
    });

    let mut response_futures = Vec::new();
    for _ in 0..6 {
        let interface = interface1.clone();
        let future = tokio::spawn(async move {
            interface
                .request(peer2, TestRequest::Slow { delay_ms: 100 })
                .await
        });
        response_futures.push(future);
    }

    for future in response_futures {
        let response = future.await.unwrap().unwrap();
        assert_eq!(response, TestResponse::SlowDone);
    }

    let max_seen = max_concurrent_seen.load(Ordering::SeqCst);

    assert!(max_seen <= 3, "Max concurrent {max_seen} should be <= 3");
    assert!(max_seen >= 2, "Should have seen some concurrency");
}

#[tokio::test]
async fn test_handler_with_complex_pattern() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;
    swarm1.connect(&mut swarm2).await;

    let peer2 = *swarm2.local_peer_id();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let handler = interface2
        .on(|req: &TestRequest| match req {
            TestRequest::Work { id, .. } => id % 2 == 0,
            _ => false,
        })
        .into_stream()
        .await
        .unwrap();

    tokio::spawn(async move {
        handler
            .respond_with_concurrent(None, |(_, req)| async move {
                match req {
                    TestRequest::Work { id, .. } => TestResponse::WorkDone { id },
                    _ => unreachable!(),
                }
            })
            .await;
    });

    let response = interface1
        .request(
            peer2,
            TestRequest::Work {
                id: 2,
                data: "test".to_string(),
            },
        )
        .await
        .unwrap();

    assert_eq!(response, TestResponse::WorkDone { id: 2 });

    let result = interface1
        .request(
            peer2,
            TestRequest::Work {
                id: 3,
                data: "test".to_string(),
            },
        )
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_handler_stream_manual_processing() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;
    swarm1.connect(&mut swarm2).await;

    let peer2 = *swarm2.local_peer_id();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let mut handler_stream = interface2
        .on(|req: &TestRequest| matches!(req, TestRequest::Echo(_)))
        .buffer_size(16)
        .into_stream()
        .await
        .unwrap();

    tokio::spawn(async move {
        while let Some(result) = handler_stream.next().await {
            match result {
                Ok(inbound) => {
                    let response = match inbound.request {
                        TestRequest::Echo(msg) => TestResponse::Echo(msg.to_uppercase()),
                        _ => unreachable!(),
                    };

                    interface2
                        .respond(inbound.request_id, inbound.channel, response)
                        .await
                        .unwrap();
                }
                Err(e) => {
                    eprintln!("Handler error: {e:?}");
                }
            }
        }
    });

    let response = interface1
        .request(peer2, TestRequest::Echo("hello".to_string()))
        .await
        .unwrap();

    assert_eq!(response, TestResponse::Echo("HELLO".to_string()));
}

#[tokio::test]
async fn test_handler_unregistration() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;
    swarm1.connect(&mut swarm2).await;

    let peer2 = *swarm2.local_peer_id();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let handler = interface2
        .on(|req: &TestRequest| matches!(req, TestRequest::Ping(_)))
        .into_stream()
        .await
        .unwrap();

    // NOTE: Handler is dropped after use and should unregister
    let handler_task = tokio::spawn(async move {
        let mut handler = handler;
        if let Some(Ok(inbound)) = handler.next().await {
            interface2
                .respond(
                    inbound.request_id,
                    inbound.channel,
                    TestResponse::Pong("first".to_string()),
                )
                .await
                .unwrap();
        }
    });

    let response = interface1
        .request(peer2, TestRequest::Ping("test".to_string()))
        .await
        .unwrap();

    assert_eq!(response, TestResponse::Pong("first".to_string()));

    handler_task.await.unwrap();

    let result = interface1
        .request(peer2, TestRequest::Ping("test2".to_string()))
        .await;

    assert!(
        matches!(result, Err(RequestResponseError::Request(_))),
        "Unexpected Result: {result:?}"
    );
}

#[tokio::test]
async fn test_duplicate_handlers() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;
    swarm1.connect(&mut swarm2).await;

    let peer2 = *swarm2.local_peer_id();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    // NOTE: We're creating multiple handlers for the same pattern.
    // Only the first should handle requests.
    let handler1 = interface2
        .on(|req: &TestRequest| matches!(req, TestRequest::Echo(_)))
        .into_stream()
        .await
        .unwrap();

    let handler2 = interface2
        .on(|req: &TestRequest| matches!(req, TestRequest::Echo(_)))
        .into_stream()
        .await
        .unwrap();

    tokio::spawn(async move {
        handler1
            .respond_with_concurrent(Some(1), move |(_, req)| async move {
                match req {
                    TestRequest::Echo(msg) => TestResponse::Echo(format!("1: {msg}")),
                    _ => unreachable!(),
                }
            })
            .await;
    });

    tokio::spawn(async move {
        handler2
            .respond_with_concurrent(Some(1), move |(_, req)| async move {
                match req {
                    TestRequest::Echo(msg) => TestResponse::Echo(format!("2: {msg}")),
                    _ => unreachable!(),
                }
            })
            .await;
    });

    let mut response_futures = Vec::new();
    for i in 0..5 {
        let interface1_clone = interface1.clone();
        let future = tokio::spawn(async move {
            interface1_clone
                .request(peer2, TestRequest::Echo(format!("msg{i}")))
                .await
        });
        response_futures.push(future);
    }

    for future in response_futures {
        let response = future.await.unwrap().unwrap();
        match response {
            TestResponse::Echo(msg) => assert!(msg.starts_with("1: ")),
            _ => panic!("Unexpected response"),
        }
    }
}

#[tokio::test]
async fn test_concurrent_request_handling() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;
    swarm1.connect(&mut swarm2).await;

    let peer2 = *swarm2.local_peer_id();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let concurrent_count = Arc::new(AtomicUsize::new(0));
    let max_concurrent_seen = Arc::new(AtomicUsize::new(0));

    let concurrent_count_clone = concurrent_count.clone();
    let max_concurrent_seen_clone = max_concurrent_seen.clone();

    let handler = interface2
        .on(|req: &TestRequest| matches!(req, TestRequest::Slow { .. }))
        .into_stream()
        .await
        .unwrap();

    tokio::spawn(async move {
        handler
            .respond_with_concurrent(None, move |(_, req)| {
                let concurrent_count = concurrent_count_clone.clone();
                let max_concurrent_seen = max_concurrent_seen_clone.clone();

                async move {
                    let current = concurrent_count.fetch_add(1, Ordering::SeqCst) + 1;

                    let mut max_seen = max_concurrent_seen.load(Ordering::SeqCst);
                    while current > max_seen {
                        match max_concurrent_seen.compare_exchange_weak(
                            max_seen,
                            current,
                            Ordering::SeqCst,
                            Ordering::SeqCst,
                        ) {
                            Ok(_) => break,
                            Err(x) => max_seen = x,
                        }
                    }

                    match req {
                        TestRequest::Slow { delay_ms } => {
                            sleep(Duration::from_millis(delay_ms)).await;
                            concurrent_count.fetch_sub(1, Ordering::SeqCst);
                            TestResponse::SlowDone
                        }
                        _ => unreachable!(),
                    }
                }
            })
            .await;
    });

    // Send many requests
    let mut response_futures = Vec::new();
    for _ in 0..20 {
        let interface1_clone = interface1.clone();
        let future = tokio::spawn(async move {
            interface1_clone
                .request(peer2, TestRequest::Slow { delay_ms: 50 })
                .await
        });
        response_futures.push(future);
    }

    for future in response_futures {
        let response = future.await.unwrap().unwrap();
        assert_eq!(response, TestResponse::SlowDone);
    }

    let max_seen = max_concurrent_seen.load(Ordering::SeqCst);

    assert!(
        max_seen >= 2,
        "Should have seen some concurrency, but max was {max_seen}"
    );
}

#[tokio::test]
async fn test_request_without_handlers() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;
    swarm1.connect(&mut swarm2).await;

    let peer2 = *swarm2.local_peer_id();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (_, driver2) = TestInterface::create(swarm2);

    tokio::spawn(driver1.run());
    tokio::spawn(driver2.run());

    let result = interface1
        .request(peer2, TestRequest::Ping("test".to_string()))
        .await;

    assert!(
        matches!(result, Err(RequestResponseError::Request(_))),
        "Unexpected Response: {result:?}"
    );
}

#[tokio::test]
async fn test_request_after_disconnect() {
    let mut swarm1 = create_test_swarm();
    let mut swarm2 = create_test_swarm();

    swarm1.listen().with_memory_addr_external().await;
    swarm2.listen().with_memory_addr_external().await;
    swarm1.connect(&mut swarm2).await;

    let peer2 = *swarm2.local_peer_id();

    let (interface1, driver1) = TestInterface::create(swarm1);
    let (interface2, driver2) = TestInterface::create(swarm2);

    let driver1_handle = tokio::spawn(driver1.run());

    let handler = interface2
        .on(|req: &TestRequest| matches!(req, TestRequest::Ping(_)))
        .into_stream()
        .await
        .unwrap();

    let handler_task = tokio::spawn(async move {
        handler
            .respond_with_concurrent(
                Some(1),
                |_| async move { TestResponse::Pong("ok".to_string()) },
            )
            .await;
    });

    // NOTE: Run the driver for a short duration so they connect and then drop it
    let _ = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_millis(100), driver2.run()).await
    })
    .await;

    let result = interface1
        .request(peer2, TestRequest::Ping("test".to_string()))
        .await;

    assert!(
        matches!(result, Err(RequestResponseError::Request(_))),
        "Unexpected Response: {result:?}"
    );
    driver1_handle.abort();
    handler_task.abort();
}
