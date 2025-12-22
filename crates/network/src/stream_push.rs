//! Streaming utilities over libp2p.
//!
//! Defines traits for sending and receiving custom stream protocols. The current
//! implementation focuses on a tensor streaming protocol but can be extended to
//! other data types in the future.

// TODO: Decide whether to model this as an abstract stream interface with the protocol identifier as an argument or

use futures_util::{Stream, StreamExt};
use libp2p::{PeerId, StreamProtocol};
use libp2p_stream::{AlreadyRegistered, Control, OpenStreamError};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::compat::FuturesAsyncReadCompatExt;

use crate::utils::{FixedAsyncRead, FixedAsyncWrite};

/// The protocol identifier for Hypha's tensor streaming protocol.
///
/// This constant defines the libp2p protocol string used for tensor data streaming
/// between peers. It follows the libp2p convention of using a path-like identifier.
const TENSOR_STREAM_PROTOCOL: StreamProtocol = StreamProtocol::new("/hypha-tensor-stream/push/2");

/// The fixed header length used for announcing payload size.
const PAYLOAD_LENGTH_HEADER_SIZE: usize = size_of::<u64>();

/// Base trait for accessing libp2p stream control functionality.
/// Meant for pushing data from one peer to another.
/// The sending peer is expected to implement StreamPushSenderInterface,
/// the receiving peer is expected to implement StreamPushReceiverInterface.
///
/// This trait provides access to the libp2p-stream `Control` object, which
/// is used to manage custom streaming protocols. It serves as the foundation
/// for both sending and receiving stream interfaces.
pub trait StreamPushInterface {
    /// Returns the stream control handle.
    ///
    /// This provides access to the libp2p-stream control interface for
    /// managing custom protocols and stream operations.
    fn stream_control(&self) -> Control;
}

/// Trait for receiving incoming streams on the tensor protocol.
///
/// This trait extends [`StreamInterface`] to provide functionality for
/// accepting incoming streams from other peers. It handles the protocol
/// registration and provides access to the stream of incoming connections.
pub trait StreamPushReceiverInterface: StreamPushInterface {
    /// Accept incoming streams.
    ///
    /// This method registers the streaming protocol and returns a stream of
    /// framed incoming connections. The returned reader verifies a trailing
    /// size marker and sends an ACK back to the sender before yielding EOF.
    ///
    /// # Returns
    ///
    /// * `Ok(Stream)` - A stream of framed incoming connections
    /// * `Err(AlreadyRegistered)` - The protocol was already registered
    ///
    /// # Errors
    ///
    /// Returns [`AlreadyRegistered`] if the protocol has already been
    /// registered with the stream control.
    fn streams_push(
        &self,
    ) -> Result<
        impl Stream<
            Item = (
                PeerId,
                FixedAsyncRead<impl AsyncRead + Send + Unpin + 'static>,
            ),
        > + Send
        + 'static,
        AlreadyRegistered,
    > {
        let incoming = self
            .stream_control()
            .accept_with_limit(TENSOR_STREAM_PROTOCOL, Some(8))?
            .filter_map(|(peer_id, stream)| async move {
                let mut stream = stream.compat();
                let mut header = [0u8; PAYLOAD_LENGTH_HEADER_SIZE];
                if let Err(e) = stream.read_exact(&mut header).await {
                    tracing::warn!("Failed to read push header: {}", e);
                    return None;
                }
                let payload_len = u64::from_le_bytes(header);

                Some((peer_id, FixedAsyncRead::new(stream, payload_len)))
            });
        Ok(incoming)
    }
}

/// Trait for sending outgoing streams on the protocol.
///
/// This trait extends [`StreamInterface`] to provide functionality for opening
/// outgoing tensor streams to other peers. It handles the protocol negotiation
/// and stream establishment.
pub trait StreamPushSenderInterface: StreamPushInterface + Sync {
    /// Open a tensor stream to a specific peer.
    ///
    /// This method establishes a direct streaming connection to the specified
    /// peer using the tensor streaming protocol. The resulting stream can be
    /// used to send tensor data efficiently.
    ///
    /// # Arguments
    ///
    /// * `peer_id` - The peer ID to establish a stream connection with
    ///
    /// # Returns
    ///
    /// * `Ok(AsyncWrite)` - A successfully opened writer to the peer
    /// * `Err(OpenStreamError)` - An error occurred during stream establishment
    fn open_push_stream(
        &self,
        peer_id: PeerId,
        payload_len: u64,
    ) -> impl Future<
        Output = Result<FixedAsyncWrite<impl AsyncWrite + Send + Unpin + 'static>, OpenStreamError>,
    > + Send {
        async move {
            let stream = self
                .stream_control()
                .open_stream(peer_id, TENSOR_STREAM_PROTOCOL)
                .await?;
            let mut stream = stream.compat();

            stream
                .write_all(&payload_len.to_le_bytes())
                .await
                .map_err(OpenStreamError::Io)?;
            stream.flush().await.map_err(OpenStreamError::Io)?;

            Ok(FixedAsyncWrite::new(stream, payload_len))
        }
    }
}

#[cfg(test)]
mod stream_interface_tests {
    use libp2p_stream::{AlreadyRegistered, Behaviour, Control};
    use mockall::mock;

    use super::*;

    mock! {
        Network {}

        impl StreamPushInterface for Network {
            fn stream_control(&self) -> Control;
        }

        impl StreamPushReceiverInterface for Network {}
        impl StreamPushSenderInterface for Network {}

    }

    #[test]
    fn test_stream_receiver_accept_twice() {
        let behaviour = Behaviour::new();
        let control = behaviour.new_control();

        let mut mock = MockNetwork::new();
        mock.expect_stream_control().return_const(control.clone());

        let stream1 = mock.streams_push();
        assert!(stream1.is_ok());

        let stream2 = mock.streams_push();
        assert!(matches!(stream2, Err(AlreadyRegistered)));
    }
}
