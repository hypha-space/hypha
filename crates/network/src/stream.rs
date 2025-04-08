// TODO: Decide whether to model this as an abstract stream interface with the protocol identifier as an argument or

use libp2p::{PeerId, Stream, StreamProtocol};
use libp2p_stream::{AlreadyRegistered, Control, IncomingStreams, OpenStreamError};

const TENSOR_STREAM_PROTOCOL: StreamProtocol = StreamProtocol::new("/hypha-tensor-stream");

#[allow(async_fn_in_trait)]
pub trait StreamInterface {
    fn stream_control(&self) -> Control;
}

#[allow(async_fn_in_trait)]
pub trait StreamReceiverInterface: StreamInterface {
    fn streams(&self) -> Result<IncomingStreams, AlreadyRegistered> {
        self.stream_control().accept(TENSOR_STREAM_PROTOCOL)
    }
}

#[allow(async_fn_in_trait)]
pub trait StreamSenderInterface: StreamInterface {
    async fn stream(&self, peer_id: PeerId) -> Result<Stream, OpenStreamError> {
        self.stream_control()
            .open_stream(peer_id, TENSOR_STREAM_PROTOCOL)
            .await
    }
}
