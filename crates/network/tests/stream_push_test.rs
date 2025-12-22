use futures_util::{StreamExt, pin_mut};
use hypha_network::stream_push::{
    StreamPushInterface, StreamPushReceiverInterface, StreamPushSenderInterface,
};
use libp2p::Swarm;
use libp2p_stream as stream;
use libp2p_swarm_test::SwarmExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct TestInterface {
    control: stream::Control,
}

impl StreamPushInterface for TestInterface {
    fn stream_control(&self) -> stream::Control {
        self.control.clone()
    }
}

impl StreamPushReceiverInterface for TestInterface {}
impl StreamPushSenderInterface for TestInterface {}

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
async fn push_succeeds_and_delivers_payload() {
    let (mut swarm_sender, sender_control) = create_test_swarm();
    let (mut swarm_receiver, receiver_control) = create_test_swarm();

    swarm_sender.listen().with_memory_addr_external().await;
    swarm_receiver.listen().with_memory_addr_external().await;
    swarm_sender.connect(&mut swarm_receiver).await;

    let receiver_peer = *swarm_receiver.local_peer_id();

    let sender = TestInterface {
        control: sender_control,
    };
    let receiver = TestInterface {
        control: receiver_control,
    };

    let sender_shutdown = CancellationToken::new();
    let receiver_shutdown = CancellationToken::new();

    tokio::spawn(
        TestDriver {
            swarm: swarm_sender,
            shutdown: sender_shutdown.clone(),
        }
        .run(),
    );
    tokio::spawn(
        TestDriver {
            swarm: swarm_receiver,
            shutdown: receiver_shutdown.clone(),
        }
        .run(),
    );

    let recv_task = tokio::spawn(async move {
        let incoming = receiver.streams_push().unwrap();
        pin_mut!(incoming);
        let (_peer, mut reader) = incoming.next().await.expect("incoming stream");
        let mut buf = [0u8; 4];
        reader.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, &[1, 2, 3, 4]);
    });

    let send_task = tokio::spawn(async move {
        let mut writer = sender.open_push_stream(receiver_peer, 4).await.unwrap();
        writer.write_all(&[1, 2, 3, 4]).await.unwrap();
        writer.shutdown().await.unwrap();
    });

    send_task.await.unwrap();
    recv_task.await.unwrap();

    sender_shutdown.cancel();
    receiver_shutdown.cancel();
}

#[tokio::test]
async fn push_under_sends_propagate_errors() {
    let (mut swarm_sender, sender_control) = create_test_swarm();
    let (mut swarm_receiver, receiver_control) = create_test_swarm();

    swarm_sender.listen().with_memory_addr_external().await;
    swarm_receiver.listen().with_memory_addr_external().await;
    swarm_sender.connect(&mut swarm_receiver).await;

    let receiver_peer = *swarm_receiver.local_peer_id();

    let sender = TestInterface {
        control: sender_control,
    };
    let receiver = TestInterface {
        control: receiver_control,
    };

    let sender_shutdown = CancellationToken::new();
    let receiver_shutdown = CancellationToken::new();

    tokio::spawn(
        TestDriver {
            swarm: swarm_sender,
            shutdown: sender_shutdown.clone(),
        }
        .run(),
    );
    tokio::spawn(
        TestDriver {
            swarm: swarm_receiver,
            shutdown: receiver_shutdown.clone(),
        }
        .run(),
    );

    // Receiver: accept the incoming stream and attempt to read the declared payload length.
    let recv_task = tokio::spawn(async move {
        let incoming = receiver.streams_push().unwrap();
        pin_mut!(incoming);
        let (_peer, mut reader) = incoming.next().await.expect("incoming stream");
        let mut buf = [0u8; 4];
        let err = reader.read_exact(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    });

    // Sender: announce a 4-byte payload, send only 2 bytes, expect shutdown to error.
    let send_task = tokio::spawn(async move {
        let mut writer = sender.open_push_stream(receiver_peer, 4).await.unwrap();
        writer.write_all(&[1, 2]).await.unwrap();
        let err = writer.shutdown().await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    });

    send_task.await.unwrap();
    recv_task.await.unwrap();

    sender_shutdown.cancel();
    receiver_shutdown.cancel();
}

#[tokio::test]
async fn push_receiver_aborts_sender_sees_error() {
    let (mut swarm_sender, sender_control) = create_test_swarm();
    let (mut swarm_receiver, receiver_control) = create_test_swarm();

    swarm_sender.listen().with_memory_addr_external().await;
    swarm_receiver.listen().with_memory_addr_external().await;
    swarm_sender.connect(&mut swarm_receiver).await;

    let receiver_peer = *swarm_receiver.local_peer_id();

    let sender = TestInterface {
        control: sender_control,
    };
    let receiver = TestInterface {
        control: receiver_control,
    };

    let sender_shutdown = CancellationToken::new();
    let receiver_shutdown = CancellationToken::new();

    tokio::spawn(
        TestDriver {
            swarm: swarm_sender,
            shutdown: sender_shutdown.clone(),
        }
        .run(),
    );
    tokio::spawn(
        TestDriver {
            swarm: swarm_receiver,
            shutdown: receiver_shutdown.clone(),
        }
        .run(),
    );

    let recv_task = tokio::spawn(async move {
        let incoming = receiver.streams_push().unwrap();
        pin_mut!(incoming);
        let _ = incoming.next().await.expect("incoming stream");
        // Drop without reading to simulate receiver failure.
    });

    let send_task = tokio::spawn(async move {
        let mut writer = sender.open_push_stream(receiver_peer, 4).await.unwrap();
        let err = writer.shutdown().await.unwrap_err();
        assert!(
            err.kind() == std::io::ErrorKind::BrokenPipe
                || err.kind() == std::io::ErrorKind::UnexpectedEof
        );
    });

    send_task.await.unwrap();
    recv_task.await.unwrap();

    sender_shutdown.cancel();
    receiver_shutdown.cancel();
}

#[tokio::test]
async fn push_sender_aborts_receiver_sees_error() {
    let (mut swarm_sender, sender_control) = create_test_swarm();
    let (mut swarm_receiver, receiver_control) = create_test_swarm();

    swarm_sender.listen().with_memory_addr_external().await;
    swarm_receiver.listen().with_memory_addr_external().await;
    swarm_sender.connect(&mut swarm_receiver).await;

    let receiver_peer = *swarm_receiver.local_peer_id();

    let sender = TestInterface {
        control: sender_control,
    };
    let receiver = TestInterface {
        control: receiver_control,
    };

    let sender_shutdown = CancellationToken::new();
    let receiver_shutdown = CancellationToken::new();

    tokio::spawn(
        TestDriver {
            swarm: swarm_sender,
            shutdown: sender_shutdown.clone(),
        }
        .run(),
    );
    tokio::spawn(
        TestDriver {
            swarm: swarm_receiver,
            shutdown: receiver_shutdown.clone(),
        }
        .run(),
    );

    let recv_task = tokio::spawn(async move {
        let incoming = receiver.streams_push().unwrap();
        pin_mut!(incoming);
        let (_peer, mut reader) = incoming.next().await.expect("incoming stream");
        let mut buf = [0u8; 4];
        let err = reader.read_exact(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
    });

    let send_task = tokio::spawn(async move {
        let _ = sender.open_push_stream(receiver_peer, 4).await.unwrap();
        // Drop without sending payload to simulate sender failure; receiver should error.
    });

    send_task.await.unwrap();
    recv_task.await.unwrap();

    sender_shutdown.cancel();
    receiver_shutdown.cancel();
}
