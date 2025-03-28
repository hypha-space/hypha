use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;

use libp2p::Multiaddr;
use libp2p::PeerId;
use libp2p::swarm::ConnectionId;
use libp2p::swarm::DialError;
use libp2p::swarm::NetworkBehaviour;
use libp2p::swarm::dial_opts::DialOpts;

use crate::swarm::SwarmDriver;
use crate::swarm::SwarmInterface;

pub enum DialAction {
    Dial(Multiaddr, oneshot::Sender<Result<PeerId, DialError>>),
}

#[allow(async_fn_in_trait)]
pub trait DialDriver<TBehavior>: SwarmDriver<TBehavior>
where
    TBehavior: NetworkBehaviour,
{
    fn pending_dials(
        &self,
    ) -> Arc<Mutex<HashMap<ConnectionId, oneshot::Sender<Result<PeerId, DialError>>>>>;

    async fn process_dial_action(&mut self, action: DialAction) {
        match action {
            DialAction::Dial(address, tx) => {
                tracing::info!(address=%address.clone(),"Dialing");
                let opts = DialOpts::from(address);
                let connection_id = opts.connection_id();

                if let Err(err) = self.swarm().dial(opts) {
                    let _ = tx.send(Err(err));
                } else {
                    self.pending_dials().lock().await.insert(connection_id, tx);
                }
            }
        }
    }

    async fn process_connection_established(&self, peer_id: PeerId, connection_id: &ConnectionId) {
        if let Some(dial) = self.pending_dials().lock().await.remove(connection_id) {
            let _ = dial.send(Ok(peer_id));
        }
    }
    async fn process_connection_error(&self, connection_id: &ConnectionId, error: DialError) {
        if let Some(dial) = self.pending_dials().lock().await.remove(connection_id) {
            let _ = dial.send(Err(error));
        }
    }
}

#[allow(async_fn_in_trait)]
pub trait DialInterface<TBehavior>: SwarmInterface<TBehavior>
where
    TBehavior: NetworkBehaviour,
    Self::Driver: DialDriver<TBehavior>,
{
    fn dial_action_sender(&self) -> Sender<DialAction>;

    async fn dial(&self, address: Multiaddr) -> Result<PeerId, DialError> {
        let (tx, rx) = oneshot::channel();
        tracing::info!(address=%address.clone(),"Dialing");

        self.dial_action_sender()
            .send(DialAction::Dial(address, tx))
            .await
            .map_err(|_| DialError::NoAddresses)?;

        rx.await.map_err(|_| DialError::Aborted)?
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
