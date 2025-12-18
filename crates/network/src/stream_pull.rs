//! Streaming utilities over libp2p.
//!
//! Defines traits for sending and receiving custom stream protocols. The current
//! implementation focuses on a tensor streaming protocol but can be extended to
//! other data types in the future.

// TODO: Decide whether to model this as an abstract stream interface with the protocol identifier as an argument or

use std::io;

use futures_util::{Stream, StreamExt};
use libp2p::{PeerId, StreamProtocol};
use libp2p_stream::{AlreadyRegistered, Control, OpenStreamError};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::compat::FuturesAsyncReadCompatExt;

use crate::utils::{FixedAsyncRead, FixedAsyncWrite};

/// The protocol identifier for Hypha's tensor streaming protocol.
///
/// This constant defines the libp2p protocol string used for tensor data streaming
/// between peers. It follows the libp2p convention of using a path-like identifier.
const TENSOR_STREAM_PROTOCOL: StreamProtocol = StreamProtocol::new("/hypha-tensor-stream/pull");

/// The maximum size of the request header in bytes.
///
/// This constant defines the maximum size of the request header that can be
/// deserialized from the stream. It is used to prevent excessive memory usage
/// and potential denial-of-service attacks.
const MAX_REQUEST_HEADER_SIZE: u64 = 1024 * 1024;

/// The fixed header length used for announcing payload size.
const PAYLOAD_LENGTH_HEADER_SIZE: usize = size_of::<u64>();

/// Pending pull stream that still needs a declared payload length.
pub struct IncomingPullStream<T, W> {
    peer_id: PeerId,
    request: T,
    stream: W,
}

impl<T, W> IncomingPullStream<T, W> {
    /// Returns the peer that initiated the pull.
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Returns the requested resource.
    pub fn request(&self) -> &T {
        &self.request
    }
}

impl<T, W: AsyncWrite + Unpin> IncomingPullStream<T, W> {
    /// Writes the payload length header and returns a length-checked writer.
    pub async fn create_response(
        self,
        payload_len: u64,
    ) -> io::Result<(PeerId, T, FixedAsyncWrite<W>)> {
        let mut stream = self.stream;
        stream.write_all(&payload_len.to_le_bytes()).await?;
        stream.flush().await?;

        Ok((
            self.peer_id,
            self.request,
            FixedAsyncWrite::new(stream, payload_len),
        ))
    }
}

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
    /// * `Ok(Stream)` - A stream of incoming connections
    /// * `Err(AlreadyRegistered)` - The protocol was already registered
    ///
    /// # Errors
    ///
    /// Returns [`AlreadyRegistered`] if the protocol has already been
    /// registered with the stream control.
    fn streams_pull(
        &self,
    ) -> Result<
        impl Stream<Item = IncomingPullStream<T, impl AsyncWrite + Unpin>> + Send,
        AlreadyRegistered,
    > {
        let incoming_streams = self
            .stream_control()
            .accept_with_limit(TENSOR_STREAM_PROTOCOL, Some(8))?
            .filter_map(|(peer_id, stream)| async move {
                let mut stream = stream.compat();
                let mut resource_len = [0u8; 8];
                if let Err(e) = stream.read_exact(&mut resource_len).await {
                    tracing::warn!("Failed to read resource header length: {}", e);
                    return None;
                };
                let request_len = u64::from_le_bytes(resource_len);

                if request_len >= MAX_REQUEST_HEADER_SIZE {
                    tracing::warn!("Resource header length exceeds maximum");
                    return None;
                }

                let mut request_bytes = vec![0; request_len as usize];

                if let Err(e) = stream.read_exact(&mut request_bytes).await {
                    tracing::warn!("Failed to read resource header: {}", e);
                    return None;
                };

                serde_json::from_slice(&request_bytes)
                    .map(|resource| IncomingPullStream {
                        peer_id,
                        request: resource,
                        stream,
                    })
                    .map_err(|e| {
                        tracing::warn!("Failed to deserialize resource header: {}", e);
                        e
                    })
                    .ok()
            });

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
    /// * `Ok(AsyncRead)` - A successfully opened reader from the peer
    /// * `Err(OpenStreamError)` - An error occurred during stream establishment
    fn open_pull_stream(
        &self,
        peer_id: PeerId,
        request: &T,
    ) -> impl Future<
        Output = Result<FixedAsyncRead<impl AsyncRead + Send + Unpin + 'static>, OpenStreamError>,
    > + Send {
        async move {
            let stream = self
                .stream_control()
                .open_stream(peer_id, TENSOR_STREAM_PROTOCOL)
                .await?;
            let mut stream = stream.compat();

            // Assuming that we only pull a single stream from a peer at a time,
            // we can simply send the resource name here.
            let request_bytes = serde_json::to_vec(request).expect("a serializable resource");
            let request_header = request_bytes.len().to_le_bytes();
            stream
                .write_all(&request_header)
                .await
                .map_err(OpenStreamError::Io)?;

            stream
                .write_all(&request_bytes)
                .await
                .map_err(OpenStreamError::Io)?;

            stream.flush().await.map_err(OpenStreamError::Io)?;

            let mut header = [0u8; PAYLOAD_LENGTH_HEADER_SIZE];
            stream
                .read_exact(&mut header)
                .await
                .map_err(OpenStreamError::Io)?;

            let payload_len = u64::from_le_bytes(header);

            Ok(FixedAsyncRead::new(stream, payload_len))
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
