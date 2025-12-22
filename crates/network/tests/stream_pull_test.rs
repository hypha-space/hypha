use futures_util::{StreamExt, pin_mut};
use hypha_network::stream_pull::{
    StreamPullInterface, StreamPullReceiverInterface, StreamPullSenderInterface,
};
use libp2p::Swarm;
use libp2p_stream as stream;
use libp2p_swarm_test::SwarmExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TestResource {
    dataset: String,
}

#[derive(Clone)]
struct TestInterface {
    control: stream::Control,
}

impl StreamPullInterface for TestInterface {
    fn stream_control(&self) -> stream::Control {
        self.control.clone()
    }
}

impl StreamPullReceiverInterface<TestResource> for TestInterface {}
impl StreamPullSenderInterface<TestResource> for TestInterface {}

fn create_test_swarm() -> (Swarm<stream::Behaviour>, stream::Control) {
    let mut control = None;
    let swarm = Swarm::new_ephemeral_tokio(|_| {
        let behaviour = stream::Behaviour::new();
        control = Some(behaviour.new_control());
        behaviour
    });
    (swarm, control.expect("stream control"))
}

struct TestDriver {
    swarm: Swarm<stream::Behaviour>,
    shutdown: CancellationToken,
}

impl TestDriver {
    async fn run(mut self) {
        loop {
            tokio::select! {
                _event = self.swarm.select_next_some() => {
                    // No-op: we only need the behaviour polled.
                },
                _ = self.shutdown.cancelled() => break,
            }
        }
    }
}

#[tokio::test]
async fn respond_pull_stream() {
    let (mut swarm_requester, requester_control) = create_test_swarm();
    let (mut swarm_provider, provider_control) = create_test_swarm();

    swarm_requester.listen().with_memory_addr_external().await;
    swarm_provider.listen().with_memory_addr_external().await;
    swarm_requester.connect(&mut swarm_provider).await;

    let provider_peer = *swarm_provider.local_peer_id();

    let requester = TestInterface {
        control: requester_control,
    };
    let provider = TestInterface {
        control: provider_control,
    };

    let requester_shutdown = CancellationToken::new();
    let provider_shutdown = CancellationToken::new();

    tokio::spawn(
        TestDriver {
            swarm: swarm_requester,
            shutdown: requester_shutdown.clone(),
        }
        .run(),
    );
    tokio::spawn(
        TestDriver {
            swarm: swarm_provider,
            shutdown: provider_shutdown.clone(),
        }
        .run(),
    );

    let provider_task = tokio::spawn(async move {
        let incoming = provider.streams_pull().unwrap();
        pin_mut!(incoming);
        let incoming = incoming.next().await.expect("incoming pull stream");
        let (_peer, resource, mut writer) = incoming.create_response(4).await.unwrap();
        assert_eq!(resource.dataset, "demo");

        writer.write_all(&[1, 2, 3, 4]).await.unwrap();
        writer.shutdown().await.unwrap();
    });

    let requester_task = tokio::spawn(async move {
        let mut reader = requester
            .open_pull_stream(
                provider_peer,
                &TestResource {
                    dataset: "demo".to_string(),
                },
            )
            .await
            .unwrap();

        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, &[1, 2, 3, 4]);
    });

    requester_task.await.unwrap();
    provider_task.await.unwrap();

    requester_shutdown.cancel();
    provider_shutdown.cancel();
}

#[tokio::test]
async fn pull_under_send_errors_on_both_sides() {
    let (mut swarm_requester, requester_control) = create_test_swarm();
    let (mut swarm_provider, provider_control) = create_test_swarm();

    swarm_requester.listen().with_memory_addr_external().await;
    swarm_provider.listen().with_memory_addr_external().await;
    swarm_requester.connect(&mut swarm_provider).await;

    let provider_peer = *swarm_provider.local_peer_id();

    let requester = TestInterface {
        control: requester_control,
    };
    let provider = TestInterface {
        control: provider_control,
    };

    let requester_shutdown = CancellationToken::new();
    let provider_shutdown = CancellationToken::new();

    tokio::spawn(
        TestDriver {
            swarm: swarm_requester,
            shutdown: requester_shutdown.clone(),
        }
        .run(),
    );
    tokio::spawn(
        TestDriver {
            swarm: swarm_provider,
            shutdown: provider_shutdown.clone(),
        }
        .run(),
    );

    // Provider: accept incoming stream, announce length, then under-send.
    let provider_task = tokio::spawn(async move {
        let incoming = provider.streams_pull().unwrap();
        pin_mut!(incoming);
        let incoming = incoming.next().await.expect("incoming pull stream");
        let (_peer, resource, mut writer) = incoming.create_response(5).await.unwrap();
        assert_eq!(resource.dataset, "demo");

        writer.write_all(&[9, 8, 7]).await.unwrap();
        let err = writer.shutdown().await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    });

    // Requester: initiate pull and expect an unexpected EOF while reading.
    let requester_task = tokio::spawn(async move {
        let mut reader = requester
            .open_pull_stream(
                provider_peer,
                &TestResource {
                    dataset: "demo".to_string(),
                },
            )
            .await
            .unwrap();

        let mut buf = [0u8; 5];
        let err = reader.read_exact(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    });

    requester_task.await.unwrap();
    provider_task.await.unwrap();

    requester_shutdown.cancel();
    provider_shutdown.cancel();
}

#[tokio::test]
async fn pull_provider_aborts_requester_errors() {
    let (mut swarm_requester, requester_control) = create_test_swarm();
    let (mut swarm_provider, provider_control) = create_test_swarm();

    swarm_requester.listen().with_memory_addr_external().await;
    swarm_provider.listen().with_memory_addr_external().await;
    swarm_requester.connect(&mut swarm_provider).await;

    let provider_peer = *swarm_provider.local_peer_id();

    let requester = TestInterface {
        control: requester_control,
    };
    let provider = TestInterface {
        control: provider_control,
    };

    let requester_shutdown = CancellationToken::new();
    let provider_shutdown = CancellationToken::new();

    tokio::spawn(
        TestDriver {
            swarm: swarm_requester,
            shutdown: requester_shutdown.clone(),
        }
        .run(),
    );
    tokio::spawn(
        TestDriver {
            swarm: swarm_provider,
            shutdown: provider_shutdown.clone(),
        }
        .run(),
    );

    let provider_task = tokio::spawn(async move {
        let incoming = provider.streams_pull().unwrap();
        pin_mut!(incoming);
        let incoming = incoming.next().await.expect("incoming pull stream");
        let (_peer, _resource, writer) = incoming.create_response(4).await.unwrap();
        drop(writer); // Drop immediately to simulate provider failure after announcing length.
    });

    let requester_task = tokio::spawn(async move {
        let mut reader = requester
            .open_pull_stream(
                provider_peer,
                &TestResource {
                    dataset: "demo".to_string(),
                },
            )
            .await
            .unwrap();

        let mut buf = [0u8; 4];
        let err = reader.read_exact(&mut buf).await.unwrap_err();
        assert!(
            err.kind() == std::io::ErrorKind::UnexpectedEof
                || err.kind() == std::io::ErrorKind::BrokenPipe
        );
    });

    requester_task.await.unwrap();
    provider_task.await.unwrap();

    requester_shutdown.cancel();
    provider_shutdown.cancel();
}

#[tokio::test]
async fn pull_requester_aborts_provider_errors() {
    let (mut swarm_requester, requester_control) = create_test_swarm();
    let (mut swarm_provider, provider_control) = create_test_swarm();

    swarm_requester.listen().with_memory_addr_external().await;
    swarm_provider.listen().with_memory_addr_external().await;
    swarm_requester.connect(&mut swarm_provider).await;

    let provider_peer = *swarm_provider.local_peer_id();

    let requester = TestInterface {
        control: requester_control,
    };
    let provider = TestInterface {
        control: provider_control,
    };

    let requester_shutdown = CancellationToken::new();
    let provider_shutdown = CancellationToken::new();

    tokio::spawn(
        TestDriver {
            swarm: swarm_requester,
            shutdown: requester_shutdown.clone(),
        }
        .run(),
    );
    tokio::spawn(
        TestDriver {
            swarm: swarm_provider,
            shutdown: provider_shutdown.clone(),
        }
        .run(),
    );

    // Provider will get an error when writing because requester aborts early.
    let provider_task = tokio::spawn(async move {
        let incoming = provider.streams_pull().unwrap();
        pin_mut!(incoming);
        let incoming = incoming.next().await.expect("incoming pull stream");
        let (_peer, _resource, mut writer) = incoming.create_response(4).await.unwrap();
        let err = writer.write_all(&[1, 2, 3, 4]).await.unwrap_err();
        assert!(
            err.kind() == std::io::ErrorKind::BrokenPipe
                || err.kind() == std::io::ErrorKind::UnexpectedEof
        );
    });

    let requester_task = tokio::spawn(async move {
        let _reader = requester
            .open_pull_stream(
                provider_peer,
                &TestResource {
                    dataset: "demo".to_string(),
                },
            )
            .await
            .unwrap();
        // Drop reader immediately to simulate requester abort.
    });

    requester_task.await.unwrap();
    provider_task.await.unwrap();

    requester_shutdown.cancel();
    provider_shutdown.cancel();
}
