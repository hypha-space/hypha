//! Kademlia distributed hash table utilities.
//!
//! Provides a thin wrapper over libp2p's Kademlia behaviour and exposes
//! asynchronous actions for query management. Other crates can use these
//! abstractions to perform peer discovery and record lookups without dealing
//! with the low level protocol details.

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt::Display,
};

use libp2p::{
    PeerId, identify,
    kad::{
        self, PeerInfo, QueryId,
        store::{self, MemoryStore},
    },
    swarm::NetworkBehaviour,
};
use tokio::sync::mpsc;
use tokio_stream::{StreamExt, wrappers::ReceiverStream};

use crate::swarm::SwarmDriver;

/// Type alias for tracking pending Kademlia queries.
///
/// Maps query IDs to channels that will receive query results as they arrive.
/// Since Kademlia queries can produce multiple results, channels are used
/// instead of oneshot channels.
pub type PendingQueries = HashMap<kad::QueryId, mpsc::Sender<KademliaResult>>;

/// Trait for network behaviours that include Kademlia functionality.
///
/// This trait provides access to the Kademlia behaviour within a composite
/// network behaviour. It's used by other traits that need to interact with
/// the Kademlia DHT protocol.
pub trait KademliaBehavior {
    /// Returns a mutable reference to the Kademlia behaviour.
    fn kademlia(&mut self) -> &mut kad::Behaviour<MemoryStore>;
}

/// Actions that can be sent to a Kademlia driver for processing.
///
/// These actions represent Kademlia DHT operations that need to be handled
/// by the network event loop. Each action includes the necessary data and
/// a response channel to communicate results back to the caller.
pub enum KademliaAction {
    /// Find providers for a specific key.
    ///
    /// This queries the DHT to find peers that are providing content for
    /// the given key.
    GetProvider(String, mpsc::Sender<KademliaResult>),

    /// Retrieve a record from the DHT.
    ///
    /// This queries the DHT to find and retrieve the value associated with
    /// the given key.
    GetRecord(String, mpsc::Sender<KademliaResult>),

    /// Store a record in the DHT.
    ///
    /// This stores the given record in the DHT, making it available for
    /// retrieval by other peers.
    PutRecord(kad::Record, mpsc::Sender<KademliaResult>),

    /// Start providing a key.
    ///
    /// This announces to the DHT that this peer can provide content for the
    /// given key.
    StartProviding(String, mpsc::Sender<KademliaResult>),

    /// Find the closest peers to a given peer ID.
    ///
    /// This queries the DHT to find peers that are closest to the target peer.
    GetClosestPeers(PeerId, mpsc::Sender<KademliaResult>),
}

/// Type alias for Kademlia operation results.
///
/// All Kademlia operations return this result type, which can contain
/// either a successful result or an error.
pub type KademliaResult = Result<KademliaOk, KademliaError>;

/// Successful results from Kademlia operations.
///
/// This enum contains the successful result data for different types
/// of Kademlia DHT operations.
pub enum KademliaOk {
    /// Result from a successful get providers query.
    GetProviders(kad::GetProvidersOk),

    /// Result from a successful start providing operation.
    StartProviding(()),

    /// Result from a successful put record operation.
    PutRecord(()),

    /// Result from a successful get record query.
    GetRecord(kad::Record),

    /// Result from a successful get closest peers query.
    GetClosestPeers(kad::GetClosestPeersOk),
}

/// Errors that can occur during Kademlia operations.
///
/// This enum wraps the various error types that can be returned by the
/// libp2p Kademlia implementation, providing a unified error type for
/// the Hypha network layer.
#[derive(Debug)]
pub enum KademliaError {
    /// Error occurred during a get providers query.
    GetProviders(kad::GetProvidersError),

    /// Error occurred during DHT storage operations.
    Store(store::Error),

    /// Error occurred during a put record operation.
    PutRecord(kad::PutRecordError),

    /// Error occurred during a get record query.
    GetRecord(kad::GetRecordError),

    /// Error occurred during a get closest peers query.
    GetClosestPeers(kad::GetClosestPeersError),

    /// Other error not covered by specific error types.
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

/// Trait for handling Kademlia DHT operations within a swarm driver.
///
/// This trait extends [`SwarmDriver`] to provide Kademlia-specific
/// functionality.
/// Implementations should track pending queries and process Kademlia-related
/// events from the libp2p swarm.
///
/// # Note
///
/// Kademlia queries can produce multiple events over time, so this trait uses
/// multi-producer single-consumer (mpsc) channels instead of oneshot channels
/// to handle the stream of results.
pub trait KademliaDriver<TBehavior>: SwarmDriver<TBehavior> + Send
where
    TBehavior: NetworkBehaviour + KademliaBehavior,
{
    /// Returns a mutable reference to the pending queries tracker.
    ///
    /// This is used to manage result channels for ongoing Kademlia queries.
    /// Since Kademlia queries can produce multiple results, mpsc channels
    /// are used to send results as they arrive.
    fn pending_queries(&mut self) -> &mut PendingQueries;

    /// Process a Kademlia action by initiating the appropriate DHT operation.
    ///
    /// This method handles all types of Kademlia actions by calling the
    /// corresponding methods on the Kademlia behaviour and tracking the
    /// resulting queries.
    ///
    /// # Arguments
    ///
    /// * `action` - The Kademlia action to process
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

    /// Handle Kademlia query results from the libp2p swarm.
    ///
    /// This method processes query results and forwards them to the appropriate
    /// pending query channels. It handles the completion state to clean up
    /// finished queries.
    ///
    /// # Arguments
    ///
    /// * `query_id` - The ID of the query that produced this result
    /// * `result` - The query result from the Kademlia behaviour
    /// * `step` - Progress information indicating if this is the final result
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

    /// Process Identify::Events and add the peer's listen addresses
    /// to the Kademlia routing table. This improves peer discovery and
    /// the overall health of the DHT.
    fn process_identify_event(&mut self, event: identify::Event) {
        match event {
            identify::Event::Received { peer_id, info, .. } => {
                // NOTE: Add known addresses of peers to the Kademlia routing table
                tracing::warn!(peer_id=%peer_id, info=?info, "Adding address to Kademlia routing table");

                self.swarm().add_external_address(info.observed_addr);

                for addr in info.listen_addrs {
                    self.swarm()
                        .behaviour_mut()
                        .kademlia()
                        .add_address(&peer_id, addr);
                }
            }
            identify::Event::Sent { peer_id, .. } => {
                tracing::trace!(peer_id=%peer_id, "Sent identify info to peer");
            }
            identify::Event::Pushed { peer_id, info, .. } => {
                tracing::warn!(peer_id=%peer_id, info=?info, "Received identify push from peer");
                // // NOTE: Handle pushed identify info similar to received info
                // for addr in info.listen_addrs {
                //     self.swarm()
                //         .behaviour_mut()
                //         .kademlia()
                //         .add_address(&peer_id, addr);
                // }
            }
            identify::Event::Error { peer_id, error, .. } => {
                tracing::warn!(peer_id=%peer_id, error=?error, "Identify protocol error");
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

/// Interface for sending Kademlia actions to a network driver.
///
/// This trait provides a high-level API for Kademlia DHT operations without
/// needing to interact directly with the libp2p swarm.
/// Implementations typically send actions through a channel to the network
/// event loop.
///
/// # Examples
///
/// ```rust,no_run
/// use hypha_network::kad::{KademliaInterface, KademliaAction};
/// use libp2p::kad;
///
/// async fn example_usage(network: impl KademliaInterface) {
///     // Store a record
///     let record = kad::Record::new(kad::RecordKey::new(&"my-key"), b"my-value".to_vec());
///     network.store(record).await.unwrap();
///
///     // Retrieve the record
///     let retrieved = network.get("my-key").await.unwrap();
///     println!("Retrieved: {:?}", String::from_utf8_lossy(&retrieved.value));
///
///     // Announce that we're providing this key
///     network.provide("my-key").await.unwrap();
///
///     // Find other providers
///     let providers = network.find_provider("my-key").await.unwrap();
///     println!("Found {} providers", providers.len());
/// }
/// ```
pub trait KademliaInterface: Sync {
    /// Send a Kademlia action to the network driver.
    ///
    /// This is the low-level method for sending actions. Most users should
    /// prefer the higher-level methods like [`store`](Self::store),
    /// [`get`](Self::get), [`provide`](Self::provide), etc.
    fn send(&self, action: KademliaAction) -> impl Future<Output = ()> + Send;

    /// Store a record in the DHT.
    ///
    /// This method stores the given record in the distributed hash table,
    /// making it available for retrieval by other peers in the network.
    ///
    /// # Arguments
    ///
    /// * `record` - The record to store in the DHT
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hypha_network::kad::{KademliaInterface, KademliaError};
    /// # use libp2p::kad;
    /// # async fn example(network: impl KademliaInterface) -> Result<(), KademliaError> {
    /// let record = kad::Record::new(
    ///     kad::RecordKey::new(&"example-key"),
    ///     b"example-value".to_vec()
    /// );
    /// network.store(record).await?;
    /// println!("Record stored successfully");
    /// # Ok(())
    /// # }
    /// ```
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

    /// Retrieve a record from the DHT.
    ///
    /// This method queries the distributed hash table to find and retrieve
    /// the value associated with the given key.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to look up in the DHT
    ///
    /// # Returns
    ///
    /// The record associated with the key, or an error if the key is not found
    /// or the operation fails.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hypha_network::kad::{KademliaInterface, KademliaError};
    /// # async fn example(network: impl KademliaInterface) -> Result<(), KademliaError> {
    /// let record = network.get("example-key").await?;
    /// println!("Retrieved value: {:?}", String::from_utf8_lossy(&record.value));
    /// # Ok(())
    /// # }
    /// ```
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

    /// Announce that this peer can provide _content_ for a key.
    ///
    /// This method announces to the DHT that this peer has _content_ available
    /// for the given key, allowing other peers to find this peer when they
    /// search for providers of that key.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to announce as being provided by this peer
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hypha_network::kad::{KademliaInterface, KademliaError};
    /// # async fn example(network: impl KademliaInterface) -> Result<(), KademliaError> {
    /// network.provide("my-content-key").await?;
    /// println!("Now providing content for my-content-key");
    /// # Ok(())
    /// # }
    /// ```
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

    /// Find peers that can provide _content_ for a key.
    ///
    /// This method queries the DHT to find all peers that have announced
    /// they can provide content for the given key.
    ///
    /// # Arguments
    ///
    /// * `key` - The key to search for providers
    ///
    /// # Returns
    ///
    /// A set of peer IDs that have announced they can provide the requested content.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hypha_network::kad::{KademliaInterface, KademliaError};
    /// # async fn example(network: impl KademliaInterface) -> Result<(), KademliaError> {
    /// let providers = network.find_provider("some-content").await?;
    /// for peer_id in providers {
    ///     println!("Peer {} provides some-content", peer_id);
    /// }
    /// # Ok(())
    /// # }
    /// ```
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

    /// Find the peers closest to a given peer ID.
    ///
    /// This method queries the DHT to find peers that are closest to the
    /// target peer ID according to the Kademlia distance metric.
    ///
    /// # Arguments
    ///
    /// * `peer_id` - The target peer ID to find closest peers for
    ///
    /// # Returns
    ///
    /// A vector of peer information for the closest peers found.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hypha_network::kad::{KademliaInterface, KademliaError};
    /// # use libp2p::PeerId;
    /// # async fn example(network: impl KademliaInterface) -> Result<(), KademliaError> {
    /// let target = PeerId::random();
    /// let closest = network.get_closest_peers(target).await?;
    /// println!("Found {} peers close to {}", closest.len(), target);
    /// # Ok(())
    /// # }
    /// ```
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
