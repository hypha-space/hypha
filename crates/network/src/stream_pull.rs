//! Streaming utilities over libp2p.
//!
//! Defines traits for sending and receiving custom stream protocols. The current
//! implementation focuses on a tensor streaming protocol but can be extended to
//! other data types in the future.

// TODO: Decide whether to model this as an abstract stream interface with the protocol identifier as an argument or

use futures_util::{AsyncReadExt, AsyncWriteExt, Stream};
use libp2p::{PeerId, StreamProtocol};
use libp2p_stream::{AlreadyRegistered, Control, OpenStreamError};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_stream::StreamExt;
use tokio_util::compat::FuturesAsyncReadCompatExt;

/// The protocol identifier for Hypha's tensor streaming protocol.
///
/// This constant defines the libp2p protocol string used for tensor data streaming
/// between peers. It follows the libp2p convention of using a path-like identifier.
const TENSOR_STREAM_PROTOCOL: StreamProtocol = StreamProtocol::new("/hypha-tensor-stream/pull");

/// The maximum size of the resource header in bytes.
///
/// This constant defines the maximum size of the resource header that can be
/// deserialized from the stream. It is used to prevent excessive memory usage
/// and potential denial-of-service attacks.
const MAX_RESOURCE_HEADER_SIZE: u64 = 1024 * 1024;

/// Base trait for accessing libp2p stream control functionality.
/// Meant for pull data from a peer.
/// The pulling peer is expected to implement StreamPullSenderInterface,
/// the pullee peer is expected to implement StreamPullReceiverInterface.
///
/// This trait provides access to the libp2p-stream `Control` object, which
/// is used to manage custom streaming protocols. It serves as the foundation
/// for both sending and receiving stream interfaces.
pub trait StreamPullInterface {
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
pub trait StreamPullReceiverInterface<T: DeserializeOwned>: StreamPullInterface {
    /// Accept incoming streams.
    ///
    /// This method registers the streaming protocol and returns a stream
    /// of incoming connections from other peers wanting to send data.
    ///
    /// # Returns
    ///
    /// * `Ok(IncomingStreams)` - A stream of incoming connections
    /// * `Err(AlreadyRegistered)` - The protocol was already registered
    ///
    /// # Errors
    ///
    /// Returns [`AlreadyRegistered`] if the protocol has already been
    /// registered with the stream control.
    fn streams_pull(
        &self,
    ) -> Result<impl Stream<Item = (PeerId, T, impl AsyncWrite)>, AlreadyRegistered> {
        let incoming_streams = self
            .stream_control()
            .accept_with_limit(TENSOR_STREAM_PROTOCOL, Some(8))?
            .then(|(peer_id, mut stream)| async move {
                let mut resource_len = [0u8; 8];
                if let Err(e) = stream.read_exact(&mut resource_len).await {
                    tracing::warn!("Failed to read resource header length: {}", e);
                    return None;
                };
                let resource_len = u64::from_le_bytes(resource_len);

                if resource_len >= MAX_RESOURCE_HEADER_SIZE {
                    tracing::warn!("Resource header length exceeds maximum");
                    return None;
                }

                let mut resource_bytes = vec![0; resource_len as usize];

                if let Err(e) = stream.read_exact(&mut resource_bytes).await {
                    tracing::warn!("Failed to read resource header: {}", e);
                    return None;
                };

                match serde_json::from_slice(&resource_bytes) {
                    Ok(resource) => Some((peer_id, resource, stream.compat())),
                    Err(e) => {
                        tracing::warn!("Failed to deserialize resource header: {}", e);
                        None
                    }
                }
            })
            .map_while(|e| e);

        Ok(incoming_streams)
    }
}

/// Trait for sending outgoing streams on the protocol.
///
/// This trait extends [`StreamInterface`] to provide functionality for opening
/// outgoing tensor streams to other peers. It handles the protocol negotiation
/// and stream establishment.
pub trait StreamPullSenderInterface<T: Serialize + Send + Sync>:
    StreamPullInterface + Sync
{
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
    /// * `Ok(Stream)` - A successfully opened stream to the peer
    /// * `Err(OpenStreamError)` - An error occurred during stream establishment
    fn stream_pull(
        &self,
        peer_id: PeerId,
        resource: &T,
    ) -> impl Future<Output = Result<impl AsyncRead + Send + 'static, OpenStreamError>> + Send {
        async move {
            let mut stream = self
                .stream_control()
                .open_stream(peer_id, TENSOR_STREAM_PROTOCOL)
                .await?;

            // Assuming that we only pull a single stream from a peer at a time,
            // we can simply send the resource name here.
            let resource_bytes = serde_json::to_vec(resource).expect("a serializable resource");
            let _ = stream.write(&resource_bytes.len().to_le_bytes()).await?;
            let _ = stream.write(&resource_bytes).await?;
            Ok(stream.compat())
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

        impl StreamPullInterface for Network {
            fn stream_control(&self) -> Control;
        }

        impl StreamPullReceiverInterface<()> for Network {}
        impl StreamPullSenderInterface<()> for Network {}

    }

    #[test]
    fn test_stream_receiver_accept_twice() {
        let behaviour = Behaviour::new();
        let control = behaviour.new_control();

        let mut mock = MockNetwork::new();
        mock.expect_stream_control().return_const(control.clone());

        let stream1 = mock.streams_pull();
        assert!(stream1.is_ok());

        let stream2 = mock.streams_pull();
        assert!(matches!(stream2, Err(AlreadyRegistered)));
    }
}
