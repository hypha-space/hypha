use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt::Display,
};

use libp2p::{
    PeerId,
    kad::{
        self, PeerInfo, QueryId,
        store::{self, MemoryStore},
    },
    swarm::NetworkBehaviour,
};
use tokio::sync::mpsc;
use tokio_stream::{StreamExt, wrappers::ReceiverStream};

use crate::swarm::SwarmDriver;

pub type PendingQueries = HashMap<kad::QueryId, mpsc::Sender<KademliaResult>>;

pub trait KademliaBehavior {
    fn kademlia(&mut self) -> &mut kad::Behaviour<MemoryStore>;
}

pub enum KademliaAction {
    GetProvider(String, mpsc::Sender<KademliaResult>),
    GetRecord(String, mpsc::Sender<KademliaResult>),
    PutRecord(kad::Record, mpsc::Sender<KademliaResult>),
    StartProviding(String, mpsc::Sender<KademliaResult>),
    GetClosestPeers(PeerId, mpsc::Sender<KademliaResult>),
}

pub type KademliaResult = Result<KademliaOk, KademliaError>;

pub enum KademliaOk {
    GetProviders(kad::GetProvidersOk),
    StartProviding(()),
    PutRecord(()),
    GetRecord(kad::Record),
    GetClosestPeers(kad::GetClosestPeersOk),
}

#[derive(Debug)]
pub enum KademliaError {
    GetProviders(kad::GetProvidersError),
    Store(store::Error),
    PutRecord(kad::PutRecordError),
    GetRecord(kad::GetRecordError),
    GetClosestPeers(kad::GetClosestPeersError),
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
            Self::GetProviders(msg) => write!(f, "Get Providers error: {msg}"),
            Self::GetRecord(msg) => write!(f, "Get Record error: {msg}"),
            Self::Store(msg) => write!(f, "Store error: {msg}"),
            Self::PutRecord(msg) => write!(f, "Put Record error: {msg}"),
            Self::GetClosestPeers(msg) => write!(f, "Get Closest Peers error: {msg}"),
            Self::Other(msg) => write!(f, "Other error: {msg}"),
        }
    }
}

pub trait KademliaDriver<TBehavior>: SwarmDriver<TBehavior> + Send
where
    TBehavior: NetworkBehaviour + KademliaBehavior,
{
    // Within Kademlia, a single query can result in multiple events.
    // Instead of using a 'oneshot' channel, we use a 'mpsc' channel to allow for multiple events to be sent.
    fn pending_queries(&mut self) -> &mut PendingQueries;

    fn process_kademlia_action(
        &mut self,
        action: KademliaAction,
    ) -> impl Future<Output = ()> + Send {
        async move {
            match action {
                KademliaAction::GetRecord(key, tx) => {
                    let query_id = self
                        .swarm()
                        .behaviour_mut()
                        .kademlia()
                        .get_record(kad::RecordKey::new(&key));

                    self.pending_queries().insert(query_id, tx);
                }
                KademliaAction::PutRecord(record, tx) => {
                    match self
                        .swarm()
                        .behaviour_mut()
                        .kademlia()
                        .put_record(record, kad::Quorum::One)
                    {
                        Ok(query_id) => {
                            self.pending_queries().insert(query_id, tx);
                        }
                        Err(err) => {
                            let _ = tx.send(Err(KademliaError::Store(err))).await;
                        }
                    }
                }
                KademliaAction::GetProvider(key, tx) => {
                    let query_id = self
                        .swarm()
                        .behaviour_mut()
                        .kademlia()
                        .get_providers(kad::RecordKey::new(&key));

                    self.pending_queries().insert(query_id, tx);
                }
                KademliaAction::StartProviding(key, tx) => {
                    match self
                        .swarm()
                        .behaviour_mut()
                        .kademlia()
                        .start_providing(kad::RecordKey::new(&key))
                    {
                        Ok(query_id) => {
                            self.pending_queries().insert(query_id, tx);
                        }
                        Err(err) => {
                            let _ = tx.send(Err(KademliaError::Store(err))).await;
                        }
                    }
                }
                KademliaAction::GetClosestPeers(peer, tx) => {
                    let query_id = self
                        .swarm()
                        .behaviour_mut()
                        .kademlia()
                        .get_closest_peers(peer);

                    self.pending_queries().insert(query_id, tx);
                }
            }
        }
    }

    fn process_kademlia_query_result(
        &mut self,
        query_id: kad::QueryId,
        result: kad::QueryResult,
        step: kad::ProgressStep,
    ) -> impl Future<Output = ()> + Send {
        async move {
            match result {
                kad::QueryResult::PutRecord(Ok(_)) => {
                    send_kademlia_result(
                        self.pending_queries(),
                        query_id,
                        step,
                        Ok(KademliaOk::PutRecord(())),
                    )
                    .await;
                }
                kad::QueryResult::PutRecord(Err(err)) => {
                    send_kademlia_result(
                        self.pending_queries(),
                        query_id,
                        step,
                        Err(KademliaError::PutRecord(err)),
                    )
                    .await;
                }
                kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(
                    kad::PeerRecord { record, .. },
                ))) => {
                    send_kademlia_result(
                        self.pending_queries(),
                        query_id,
                        step,
                        Ok(KademliaOk::GetRecord(record)),
                    )
                    .await;
                }
                kad::QueryResult::GetRecord(Err(err)) => {
                    send_kademlia_result(
                        self.pending_queries(),
                        query_id,
                        step,
                        Err(KademliaError::GetRecord(err)),
                    )
                    .await;
                }
                kad::QueryResult::GetProviders(Ok(providers)) => {
                    send_kademlia_result(
                        self.pending_queries(),
                        query_id,
                        step,
                        Ok(KademliaOk::GetProviders(providers)),
                    )
                    .await;
                }
                kad::QueryResult::GetProviders(Err(err)) => {
                    send_kademlia_result(
                        self.pending_queries(),
                        query_id,
                        step,
                        Err(KademliaError::GetProviders(err)),
                    )
                    .await;
                }
                kad::QueryResult::StartProviding(Ok(_)) => {
                    send_kademlia_result(
                        self.pending_queries(),
                        query_id,
                        step,
                        Ok(KademliaOk::StartProviding(())),
                    )
                    .await
                }
                kad::QueryResult::GetClosestPeers(Ok(closest_peers)) => {
                    send_kademlia_result(
                        self.pending_queries(),
                        query_id,
                        step,
                        Ok(KademliaOk::GetClosestPeers(closest_peers)),
                    )
                    .await;
                }
                kad::QueryResult::GetClosestPeers(Err(err)) => {
                    send_kademlia_result(
                        self.pending_queries(),
                        query_id,
                        step,
                        Err(KademliaError::GetClosestPeers(err)),
                    )
                    .await;
                }
                other => {
                    tracing::debug!("Unhandled Kademlia Queryresult: {:?}", other);
                }
            }
        }
    }
}

async fn send_kademlia_result(
    pending_queries: &mut PendingQueries,
    query_id: QueryId,
    step: kad::ProgressStep,
    result: KademliaResult,
) {
    let tx = match pending_queries.get(&query_id).cloned() {
        Some(tx) => tx,
        None => return,
    };

    let _ = tx.send(result).await;

    if step.last {
        let _ = pending_queries.remove(&query_id);
    }
}

pub trait KademliaInterface: Sync {
    fn send(&self, action: KademliaAction) -> impl Future<Output = ()> + Send;

    fn store(&self, record: kad::Record) -> impl Future<Output = Result<(), KademliaError>> + Send {
        async move {
            let (tx, rx) = mpsc::channel(1);
            tracing::info!(record=?record,"Store record", );

            self.send(KademliaAction::PutRecord(record, tx)).await;

            ReceiverStream::new(rx)
                .fold(Ok(()), |acc, result| match result {
                    Ok(KademliaOk::PutRecord(_)) => acc,
                    Err(err) => Err(err),
                    _ => Err(KademliaError::Other("Unexpected response".to_string())),
                })
                .await
        }
    }

    fn get(&self, key: &str) -> impl Future<Output = Result<kad::Record, KademliaError>> + Send {
        async move {
            let (tx, mut rx) = mpsc::channel(1);
            tracing::info!(key=%key,"Get key", );

            self.send(KademliaAction::GetRecord(key.to_string(), tx))
                .await;

            let result = rx.recv().await.ok_or_else(|| {
                KademliaError::Other("No response received from the network".to_string())
            })?;

            match result {
                Ok(KademliaOk::GetRecord(record)) => Ok(record),
                Err(err) => Err(err),
                _ => Err(KademliaError::Other("Unexpected response".to_string())),
            }
        }
    }

    fn provide(&self, key: &str) -> impl Future<Output = Result<(), KademliaError>> + Send {
        async move {
            let (tx, rx) = mpsc::channel(1);
            tracing::info!(key=%key, "Provide key");

            self.send(KademliaAction::StartProviding(key.to_string(), tx))
                .await;

            ReceiverStream::new(rx)
                .fold(Ok(()), |acc, result| match result {
                    Ok(KademliaOk::StartProviding(_)) => acc,
                    Err(err) => Err(err),
                    _ => Err(KademliaError::Other("Unexpected response".to_string())),
                })
                .await
        }
    }

    fn find_provider(
        &self,
        key: &str,
    ) -> impl Future<Output = Result<HashSet<PeerId>, KademliaError>> + Send {
        async move {
            // TODO: Determine rigth channel size, I suspect a small size like 5 is already sufficient.
            let (tx, rx) = mpsc::channel(5);
            tracing::info!(key=%key, "Find key providers");

            self.send(KademliaAction::GetProvider(key.to_string(), tx))
                .await;

            ReceiverStream::new(rx)
                .fold(Ok(HashSet::new()), |acc, result| match result {
                    Ok(KademliaOk::GetProviders(kad::GetProvidersOk::FoundProviders {
                        providers,
                        ..
                    })) => acc.map(|mut acc| {
                        acc.extend(providers);
                        acc
                    }),
                    Ok(KademliaOk::GetProviders(
                        kad::GetProvidersOk::FinishedWithNoAdditionalRecord { .. },
                    )) => acc,
                    Err(err) => Err(err),
                    _ => Err(KademliaError::Other("Unexpected response".to_string())),
                })
                .await
        }
    }

    fn get_closest_peers(
        &self,
        peer_id: PeerId,
    ) -> impl Future<Output = Result<Vec<PeerInfo>, KademliaError>> + Send {
        async move {
            let (tx, rx) = mpsc::channel(5);
            tracing::info!(peer_id=?peer_id, "Get closest peers");

            self.send(KademliaAction::GetClosestPeers(peer_id, tx))
                .await;

            ReceiverStream::new(rx)
                .fold(Ok(Vec::new()), |acc, result| match result {
                    Ok(KademliaOk::GetClosestPeers(kad::GetClosestPeersOk { peers, .. })) => acc
                        .map(|mut acc| {
                            acc.extend(peers);
                            acc
                        }),
                    Err(err) => Err(err),
                    _ => Err(KademliaError::Other("Unexpected response".to_string())),
                })
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use mockall::mock;

    use super::*;

    mock! {
        TestInterface {}

        impl KademliaInterface for TestInterface {
            async fn send(&self, action: KademliaAction);
        }
    }

    #[tokio::test]
    async fn test_kademlia_interface_store() {
        let mut mock = MockTestInterface::new();

        mock.expect_send()
            .withf(|action| matches!(action, KademliaAction::PutRecord(_, _)))
            .times(1)
            .returning(|action| {
                if let KademliaAction::PutRecord(_, tx) = action {
                    tokio::spawn(async move {
                        // Simulate a successful store operation
                        let _ = tx.send(Ok(KademliaOk::PutRecord(()))).await;
                    });
                }
            });

        let record = kad::Record::new(kad::RecordKey::new(&"foo"), vec![1, 2, 3]);
        let result = mock.store(record).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_kademlia_interface_get() {
        let mut mock = MockTestInterface::new();

        mock.expect_send()
            .withf(|action| matches!(action, KademliaAction::GetRecord(_, _)))
            .times(1)
            .returning(|action| {
                if let KademliaAction::GetRecord(key, tx) = action {
                    tokio::spawn(async move {
                        // Create a mock record and send it back
                        let record = kad::Record::new(kad::RecordKey::new(&key), vec![1, 2, 3]);
                        let _ = tx.send(Ok(KademliaOk::GetRecord(record))).await;
                    });
                }
            });

        let result = mock.get("foo").await.unwrap();

        assert!(result.key == kad::RecordKey::new(&"foo"));
        assert_eq!(result.value, &[1, 2, 3]);
    }
}
