// TODO: Decide whether to model this as an abstract stream interface with the protocol identifier as an argument or

use libp2p::PeerId;
use libp2p::Stream;
use libp2p::StreamProtocol;
use libp2p::swarm::NetworkBehaviour;
use libp2p_stream::AlreadyRegistered;
use libp2p_stream::Control;
use libp2p_stream::IncomingStreams;
use libp2p_stream::OpenStreamError;

use crate::swarm::SwarmInterface;

const TENSOR_STREAM_PROTOCOL: StreamProtocol = StreamProtocol::new("/hypha-tensor-stream");

#[allow(async_fn_in_trait)]
pub trait StreamInterface<TBehavior>: SwarmInterface<TBehavior>
where
    TBehavior: NetworkBehaviour,
{
    fn stream_control(&self) -> Control;
}

#[allow(async_fn_in_trait)]
pub trait StreamReceiverInterface<TBehavior>: StreamInterface<TBehavior>
where
    TBehavior: NetworkBehaviour,
{
    fn streams(&self) -> Result<IncomingStreams, AlreadyRegistered> {
        self.stream_control().accept(TENSOR_STREAM_PROTOCOL)
    }
}

#[allow(async_fn_in_trait)]
pub trait StreamSenderInterface<TBehavior>: StreamInterface<TBehavior>
where
    TBehavior: NetworkBehaviour,
{
    async fn stream(&self, peer_id: PeerId) -> Result<Stream, OpenStreamError> {
        self.stream_control()
            .open_stream(peer_id, TENSOR_STREAM_PROTOCOL)
            .await
    }
}
