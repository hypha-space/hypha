use std::{
    collections::HashMap,
    io::{Error as IoError, ErrorKind as IoErrorKind},
    sync::Arc,
};

use libp2p::{Multiaddr, TransportError, core::transport::ListenerId, swarm::NetworkBehaviour};
use tokio::sync::{Mutex, oneshot};

use crate::swarm::SwarmDriver;

pub type PendingListens =
    Arc<Mutex<HashMap<ListenerId, oneshot::Sender<Result<(), TransportError<IoError>>>>>>;

pub enum ListenAction {
    Listen(
        Multiaddr,
        oneshot::Sender<Result<(), TransportError<IoError>>>,
    ),
}

#[allow(async_fn_in_trait)]
pub trait ListenDriver<TBehavior>: SwarmDriver<TBehavior>
where
    TBehavior: NetworkBehaviour,
{
    fn pending_listens(&self) -> PendingListens;

    async fn process_listen_action(&mut self, action: ListenAction) {
        match action {
            ListenAction::Listen(address, tx) => {
                tracing::info!(address=%address.clone(),"Listening");

                match self.swarm().listen_on(address.clone()) {
                    Ok(listener_id) => {
                        self.pending_listens().lock().await.insert(listener_id, tx);
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                    }
                }
            }
        }
    }

    async fn process_new_listen_addr(&self, listener_id: &ListenerId) {
        if let Some(tx) = self.pending_listens().lock().await.remove(listener_id) {
            let _ = tx.send(Ok(()));
        }
    }
}

#[allow(async_fn_in_trait)]
pub trait ListenInterface {
    async fn send(&self, action: ListenAction);

    async fn listen(&self, address: Multiaddr) -> Result<(), TransportError<IoError>> {
        let (tx, rx) = oneshot::channel();
        tracing::info!(address=%address.clone(),"Listening");

        self.send(ListenAction::Listen(address, tx)).await;

        rx.await.map_err(|err| {
            TransportError::Other(IoError::new(
                IoErrorKind::Other,
                format!("Failed to recieve action response: {}", err),
            ))
        })?
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     // #[test]
//     // fn it_works() {
//     //     let result = add(2, 2);
//     //     assert_eq!(result, 4);
//     // }
// }
