use std::{collections::HashMap, io::Error as IoError};

use libp2p::{Multiaddr, TransportError, core::transport::ListenerId, swarm::NetworkBehaviour};
use tokio::sync::oneshot;

use crate::swarm::SwarmDriver;

pub type PendingListens = HashMap<ListenerId, oneshot::Sender<Result<(), TransportError<IoError>>>>;

pub enum ListenAction {
    Listen(
        Multiaddr,
        oneshot::Sender<Result<(), TransportError<IoError>>>,
    ),
}

pub trait ListenDriver<TBehavior>: SwarmDriver<TBehavior> + Send
where
    TBehavior: NetworkBehaviour,
{
    fn pending_listens(&mut self) -> &mut PendingListens;

    fn process_listen_action(&mut self, action: ListenAction) -> impl Future<Output = ()> + Send {
        async move {
            match action {
                ListenAction::Listen(address, tx) => {
                    tracing::info!(address=%address.clone(),"Listening");

                    match self.swarm().listen_on(address.clone()) {
                        Ok(listener_id) => {
                            self.pending_listens().insert(listener_id, tx);
                        }
                        Err(err) => {
                            let _ = tx.send(Err(err));
                        }
                    }
                }
            }
        }
    }

    fn process_new_listen_addr(
        &mut self,
        listener_id: &ListenerId,
    ) -> impl Future<Output = ()> + Send {
        async move {
            if let Some(tx) = self.pending_listens().remove(listener_id) {
                let _ = tx.send(Ok(()));
            }
        }
    }
}

pub trait ListenInterface: Sync {
    fn send(&self, action: ListenAction) -> impl Future<Output = ()> + Send;

    fn listen(
        &self,
        address: Multiaddr,
    ) -> impl Future<Output = Result<(), TransportError<IoError>>> + Send {
        async move {
            let (tx, rx) = oneshot::channel();
            tracing::info!(address=%address.clone(),"Listening");

            self.send(ListenAction::Listen(address, tx)).await;

            rx.await.map_err(|err| {
                TransportError::Other(IoError::other(format!(
                    "Failed to recieve action response: {err}"
                )))
            })?
        }
    }
}

#[cfg(test)]
mod listen_interface_tests {
    use libp2p::{Multiaddr, core::transport::TransportError};
    use mockall::mock;

    use super::*;

    mock! {
        TestListenInterface {}

        impl ListenInterface for TestListenInterface {
            async fn send(&self, action: ListenAction);
        }
    }

    #[tokio::test]
    async fn test_listen_interface_success() {
        let mut mock = MockTestListenInterface::new();
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/4321".parse().unwrap();
        let addr_clone = addr.clone();

        mock.expect_send()
            .withf(move |action| matches!(action, ListenAction::Listen(a, _) if a == &addr_clone))
            .times(1)
            .returning(|action| match action {
                ListenAction::Listen(_, tx) => {
                    tokio::spawn(async move {
                        let _ = tx.send(Ok(()));
                    });
                }
            });

        let result = mock.listen(addr.clone()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_listen_interface_aborted() {
        let mut mock = MockTestListenInterface::new();
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/4321".parse().unwrap();
        let addr_clone = addr.clone();

        mock.expect_send()
            .withf(move |action| matches!(action, ListenAction::Listen(a, _) if a == &addr_clone))
            .times(1)
            .returning(|_action| {});

        let err = mock.listen(addr.clone()).await.unwrap_err();

        match err {
            TransportError::Other(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::Other);
            }
            _ => panic!("Expected Other error"),
        }
    }
}
