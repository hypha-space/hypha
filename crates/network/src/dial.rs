use std::collections::HashMap;

use libp2p::{
    Multiaddr, PeerId,
    swarm::{ConnectionId, DialError, NetworkBehaviour, dial_opts::DialOpts},
};
use tokio::sync::oneshot;

use crate::swarm::SwarmDriver;

pub type PendingDials = HashMap<ConnectionId, oneshot::Sender<Result<PeerId, DialError>>>;

pub enum DialAction {
    Dial(Multiaddr, oneshot::Sender<Result<PeerId, DialError>>),
}

#[allow(async_fn_in_trait)]
pub trait DialDriver<TBehavior>: SwarmDriver<TBehavior>
where
    TBehavior: NetworkBehaviour,
{
    fn pending_dials(&mut self) -> &mut PendingDials;

    async fn process_dial_action(&mut self, action: DialAction) {
        match action {
            DialAction::Dial(address, tx) => {
                tracing::info!(address=%address.clone(),"Dialing");
                let opts = DialOpts::from(address);
                let connection_id = opts.connection_id();

                if let Err(err) = self.swarm().dial(opts) {
                    let _ = tx.send(Err(err));
                } else {
                    self.pending_dials().insert(connection_id, tx);
                }
            }
        }
    }

    async fn process_connection_established(
        &mut self,
        peer_id: PeerId,
        connection_id: &ConnectionId,
    ) {
        if let Some(dial) = self.pending_dials().remove(connection_id) {
            let _ = dial.send(Ok(peer_id));
        }
    }
    async fn process_connection_error(&mut self, connection_id: &ConnectionId, error: DialError) {
        if let Some(dial) = self.pending_dials().remove(connection_id) {
            let _ = dial.send(Err(error));
        }
    }
}

#[allow(async_fn_in_trait)]
pub trait DialInterface {
    async fn send(&self, action: DialAction);

    async fn dial(&self, address: Multiaddr) -> Result<PeerId, DialError> {
        let (tx, rx) = oneshot::channel();
        tracing::info!(address=%address.clone(),"Dialing");

        self.send(DialAction::Dial(address, tx)).await;

        rx.await.map_err(|_| DialError::Aborted)?
    }
}

#[cfg(test)]
mod dial_interface_tests {
    use super::*;
    use libp2p::Multiaddr;
    use libp2p::PeerId;
    use libp2p::swarm::DialError;
    use mockall::mock;

    mock! {
        NetworkInterface {}

        impl DialInterface for NetworkInterface {
            async fn send(&self, action: DialAction);
        }
    }

    #[tokio::test]
    async fn test_dial_interface_success() {
        let mut mock = MockNetworkInterface::new();
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/1234".parse().unwrap();
        let addr_clone = addr.clone();

        mock.expect_send()
            .withf(move |action| matches!(action, DialAction::Dial(a, _) if a == &addr_clone))
            .times(1)
            .returning(|action| {
                let DialAction::Dial(_, tx) = action;
                tokio::spawn(async move {
                    let peer = PeerId::random();
                    let _ = tx.send(Ok(peer));
                });
            });

        // calling the real `.dial` on our mock
        let result = mock.dial(addr.clone()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_dial_interface_aborted() {
        let mut mock = MockNetworkInterface::new();
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/1234".parse().unwrap();
        let addr_clone = addr.clone();

        mock.expect_send()
            .withf(move |action| matches!(action, DialAction::Dial(a, _) if a == &addr_clone))
            .times(1)
            .returning(|_action| {});

        let err = mock.dial(addr.clone()).await.unwrap_err();
        assert!(matches!(err, DialError::Aborted));
    }
}
