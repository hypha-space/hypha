//! Streaming utilities over libp2p.
//!
//! Defines traits for sending and receiving custom stream protocols. The current
//! implementation focuses on a tensor streaming protocol but can be extended to
//! other data types in the future.

// TODO: Decide whether to model this as an abstract stream interface with the protocol identifier as an argument or

use libp2p::{PeerId, Stream, StreamProtocol};
use libp2p_stream::{AlreadyRegistered, Control, IncomingStreams, OpenStreamError};

/// The protocol identifier for Hypha's tensor streaming protocol.
///
/// This constant defines the libp2p protocol string used for tensor data streaming
/// between peers. It follows the libp2p convention of using a path-like identifier.
const TENSOR_STREAM_PROTOCOL: StreamProtocol = StreamProtocol::new("/hypha-tensor-stream");

/// Base trait for accessing libp2p stream control functionality.
///
/// This trait provides access to the libp2p-stream `Control` object, which
/// is used to manage custom streaming protocols. It serves as the foundation
/// for both sending and receiving stream interfaces.
pub trait StreamInterface {
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
pub trait StreamReceiverInterface: StreamInterface {
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
    fn streams(&self) -> Result<IncomingStreams, AlreadyRegistered> {
        self.stream_control().accept(TENSOR_STREAM_PROTOCOL)
    }
}

/// Trait for sending outgoing streams on the protocol.
///
/// This trait extends [`StreamInterface`] to provide functionality for opening
/// outgoing tensor streams to other peers. It handles the protocol negotiation
/// and stream establishment.
pub trait StreamSenderInterface: StreamInterface + Sync {
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
    fn stream(
        &self,
        peer_id: PeerId,
    ) -> impl Future<Output = Result<Stream, OpenStreamError>> + Send {
        async move {
            self.stream_control()
                .open_stream(peer_id, TENSOR_STREAM_PROTOCOL)
                .await
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

        impl StreamInterface for Network {
            fn stream_control(&self) -> Control;
        }

        impl StreamReceiverInterface for Network {}
        impl StreamSenderInterface for Network {}

    }

    #[test]
    fn test_stream_receiver_accept_twice() {
        let behaviour = Behaviour::new();
        let control = behaviour.new_control();

        let mut mock = MockNetwork::new();
        mock.expect_stream_control().return_const(control.clone());

        let stream1 = mock.streams();
        assert!(stream1.is_ok());

        let stream2 = mock.streams();
        assert!(matches!(stream2, Err(AlreadyRegistered)));
    }
}
