// TODO: Decide whether to model this as an abstract stream interface with the protocol identifier as an argument or

use libp2p::{PeerId, Stream, StreamProtocol};
use libp2p_stream::{AlreadyRegistered, Control, IncomingStreams, OpenStreamError};

const TENSOR_STREAM_PROTOCOL: StreamProtocol = StreamProtocol::new("/hypha-tensor-stream");

pub trait StreamInterface {
    fn stream_control(&self) -> Control;
}

pub trait StreamReceiverInterface: StreamInterface {
    fn streams(&self) -> Result<IncomingStreams, AlreadyRegistered> {
        self.stream_control().accept(TENSOR_STREAM_PROTOCOL)
    }
}

pub trait StreamSenderInterface: StreamInterface + Sync {
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
