use std::collections::HashMap;
use std::collections::HashSet;
use std::error::Error;
use std::fmt::Display;
use std::sync::Arc;

use libp2p::kad::PutRecordResult;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use libp2p::PeerId;
use libp2p::kad;
use libp2p::kad::QueryId;
use libp2p::kad::store;
use libp2p::kad::store::MemoryStore;
use libp2p::swarm::NetworkBehaviour;

use crate::swarm::SwarmDriver;
use crate::swarm::SwarmInterface;

pub trait KademliaBehavior {
    fn kademlia(&mut self) -> &mut kad::Behaviour<MemoryStore>;
}

pub enum KademliaAction {
    GetProvider(String, Sender<KademliaResult>),
    GetRecord(String, Sender<KademliaResult>),
    PutRecord(kad::Record, Sender<KademliaResult>),
    // TODO: Consider removing
    PutProvider(String, Sender<KademliaResult>),
}

pub type KademliaResult = Result<KademliaOk, KademliaError>;

pub enum KademliaOk {
    GetProvidersOk(kad::GetProvidersOk),
    PutProviderOk(()),
    PutRecordOk(()),
    GetRecordOk(kad::Record),
}

#[derive(Debug)]
pub enum KademliaError {
    GetProvidersError(kad::GetProvidersError),
    StoreError(store::Error),
    GetRecordError(kad::GetRecordError),
    Other(String),
}

impl Error for KademliaError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

impl Display for KademliaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GetProvidersError(msg) => write!(f, "Get Providers error: {}", msg),
            Self::GetRecordError(msg) => write!(f, "Get Record error: {}", msg),
            Self::StoreError(msg) => write!(f, "Store error: {}", msg),
            Self::Other(msg) => write!(f, "Other error: {}", msg),
        }
    }
}

#[allow(async_fn_in_trait)]
pub trait KademliaDriver<TBehavior>: SwarmDriver<TBehavior>
where
    TBehavior: NetworkBehaviour + KademliaBehavior,
{
    fn pending_queries(&self) -> Arc<Mutex<HashMap<QueryId, Sender<KademliaResult>>>>;

    async fn process_kademlia_action(&mut self, action: KademliaAction) {
        match action {
            KademliaAction::GetRecord(key, tx) => {
                let query_id = self
                    .swarm()
                    .behaviour_mut()
                    .kademlia()
                    .get_record(kad::RecordKey::new(&key));

                self.pending_queries().lock().await.insert(query_id, tx);
            }
            KademliaAction::PutRecord(record, tx) => {
                match self
                    .swarm()
                    .behaviour_mut()
                    .kademlia()
                    .put_record(record, kad::Quorum::One)
                {
                    Ok(query_id) => {
                        self.pending_queries().lock().await.insert(query_id, tx);
                    }
                    Err(err) => {
                        tx.send(Err(KademliaError::StoreError(err).into()))
                            .await
                            .ok();
                    }
                }
            }
            KademliaAction::GetProvider(key, tx) => {
                let query_id = self
                    .swarm()
                    .behaviour_mut()
                    .kademlia()
                    .get_providers(kad::RecordKey::new(&key));

                self.pending_queries().lock().await.insert(query_id, tx);
            }
            KademliaAction::PutProvider(key, tx) => {
                match self
                    .swarm()
                    .behaviour_mut()
                    .kademlia()
                    .start_providing(kad::RecordKey::new(&key))
                {
                    Ok(query_id) => {
                        self.pending_queries().lock().await.insert(query_id, tx);
                    }
                    Err(err) => {
                        tx.send(Err(KademliaError::StoreError(err).into()))
                            .await
                            .ok();
                    }
                }
            }
        }
    }

    async fn process_kademlia_query_result(
        &mut self,
        query_id: kad::QueryId,
        result: kad::QueryResult,
        step: kad::ProgressStep,
    ) {
        let tx = match self.pending_queries().lock().await.get(&query_id).cloned() {
            Some(tx) => tx,
            None => return,
        };

        match result {
            kad::QueryResult::PutRecord(Ok(_)) => {
                let _ = tx.send(Ok(KademliaOk::PutRecordOk(()))).await;
            }
            kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(kad::PeerRecord {
                record,
                ..
            }))) => {
                let _ = tx.send(Ok(KademliaOk::GetRecordOk(record))).await;
            }
            kad::QueryResult::GetRecord(Err(err)) => {
                let _ = tx.send(Err(KademliaError::GetRecordError(err))).await;
            }
            kad::QueryResult::GetProviders(Ok(providers)) => {
                tracing::info!(providers=?providers, "GetProviders");
                let _ = tx.send(Ok(KademliaOk::GetProvidersOk(providers))).await;
            }
            kad::QueryResult::GetProviders(Err(err)) => {
                tracing::info!(err=?err, "GetProviders");
                let _ = tx.send(Err(KademliaError::GetProvidersError(err))).await;
            }

            kad::QueryResult::StartProviding(Ok(_)) => {
                let _ = tx.send(Ok(KademliaOk::PutProviderOk(()))).await;
            }
            _ => {
                tracing::debug!("Unhandled Kademlia Queryresult.")
            }
        }

        // Drop channel
        if step.last {
            if let Some(tx) = self.pending_queries().lock().await.remove(&query_id) {
                drop(tx);
            }
        }
    }
}

#[allow(async_fn_in_trait)]
pub trait KademliaInterface<TBehavior>: SwarmInterface<TBehavior>
where
    TBehavior: NetworkBehaviour + KademliaBehavior,
    Self::Driver: KademliaDriver<TBehavior>,
{
    fn kademlia_action_sender(&self) -> Sender<KademliaAction>;

    async fn store(&self, record: kad::Record) -> Result<(), KademliaError> {
        let (tx, rx) = channel(1);
        tracing::info!(record=?record,"Store record", );

        self.kademlia_action_sender()
            .send(KademliaAction::PutRecord(record, tx))
            .await
            .map_err(|_| KademliaError::Other("Failed to send PutRecord action".to_string()))?;

        ReceiverStream::new(rx)
            .fold(Ok(()), |acc, result| match result {
                Ok(KademliaOk::PutRecordOk(_)) => acc,
                Err(err) => Err(err),
                _ => Err(KademliaError::Other("Unexpected response".to_string())),
            })
            .await
    }

    async fn get(&self, key: &str) -> Result<kad::Record, KademliaError> {
        let (tx, mut rx) = channel(1);
        tracing::info!(key=%key,"Get key", );

        self.kademlia_action_sender()
            .send(KademliaAction::GetRecord(key.to_string(), tx))
            .await
            .map_err(|_| KademliaError::Other("Failed to send GetRecord action".to_string()))?;

        // Get the firt record from the stream
        let result = rx.recv().await.ok_or_else(|| {
            KademliaError::Other("No response received from the network".to_string())
        })?;

        match result {
            Ok(KademliaOk::GetRecordOk(record)) => Ok(record),
            Err(err) => Err(err),
            _ => Err(KademliaError::Other("Unexpected response".to_string())),
        }
    }

    async fn provide(&self, key: &str) -> Result<(), KademliaError> {
        let (tx, rx) = channel(1);
        tracing::info!(key=%key,"Provide key", );

        self.kademlia_action_sender()
            .send(KademliaAction::PutProvider(key.to_string(), tx))
            .await
            .map_err(|_| KademliaError::Other("Failed to send Provide action".to_string()))?;

        ReceiverStream::new(rx)
            .fold(Ok(()), |acc, result| match result {
                Ok(KademliaOk::PutProviderOk(_)) => acc,
                Err(err) => Err(err),
                _ => Err(KademliaError::Other("Unexpected response".to_string())),
            })
            .await
    }

    async fn find_provider(&self, key: &str) -> Result<HashSet<PeerId>, KademliaError> {
        // TODO: Determine rigth channel size, I suspect a small size like 5 is already sufficient.
        let (tx, rx) = channel(5);
        tracing::info!(key=%key,"Find key providers",);

        self.kademlia_action_sender()
            .send(KademliaAction::GetProvider(key.to_string(), tx))
            .await
            .map_err(|_| KademliaError::Other("Failed to send GetProvider action".to_string()))?;

        ReceiverStream::new(rx)
            .fold(Ok(HashSet::new()), |acc, result| match result {
                Ok(KademliaOk::GetProvidersOk(kad::GetProvidersOk::FoundProviders {
                    providers,
                    ..
                })) => acc.map(|mut acc| {
                    acc.extend(providers);
                    acc
                }),
                Ok(KademliaOk::GetProvidersOk(
                    kad::GetProvidersOk::FinishedWithNoAdditionalRecord { .. },
                )) => acc,
                Err(err) => Err(err),
                // We're using one pending action map thus need to handle the whole `KademliaResult` though any reulst other then `GetProviders` related ones would be unexpected and an error.
                _ => Err(KademliaError::Other(
                    "Received unexpected response".to_string(),
                )),
            })
            .await
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
