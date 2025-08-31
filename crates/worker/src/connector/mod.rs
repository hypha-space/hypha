//! Connector module: pluggable connectors for fetching, sending, and receiving data.

use std::{io, pin::Pin, sync::Arc};

use futures::{
    Stream, StreamExt, TryStreamExt,
    io::{AsyncRead, AsyncWrite},
};
use hf_hub::api::tokio::ApiBuilder;
use hypha_messages::{Fetch, Receive, Reference, SelectionStrategy, Send as SendRef};
use hypha_network::stream::{StreamInterface, StreamReceiverInterface, StreamSenderInterface};
use libp2p::PeerId;
use libp2p_stream::{AlreadyRegistered, OpenStreamError};
use reqwest;
use thiserror::Error;
use tokio_util::{compat::TokioAsyncReadCompatExt, io::StreamReader};

pub type BoxAsyncRead = Pin<Box<dyn AsyncRead + Send>>;
pub type BoxAsyncWrite = Pin<Box<dyn AsyncWrite + Send + Unpin>>;

#[derive(Debug, Clone)]
pub struct ItemMeta {
    pub kind: &'static str,
    pub name: String,
}

pub struct ReadItem {
    pub meta: ItemMeta,
    pub reader: BoxAsyncRead,
}

pub struct WriteItem {
    pub meta: ItemMeta,
    pub writer: BoxAsyncWrite,
}

pub type ReadItemStream = Pin<Box<dyn Stream<Item = Result<ReadItem, io::Error>> + Send>>;
pub type WriteItemStream = Pin<Box<dyn Stream<Item = Result<WriteItem, ConnectorError>> + Send>>;

#[derive(Debug, Error)]
pub enum ConnectorError {
    #[error("Open stream error: {0}")]
    OpenStream(#[from] OpenStreamError),
    #[error("Stream already registered: {0}")]
    AlreadyRegistered(#[from] AlreadyRegistered),
    #[error("HuggingFace API error: {0}")]
    HuggingFace(#[from] hf_hub::api::tokio::ApiError),
    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("unsupported fetch reference: {0:?}")]
    UnsupportedFetch(Reference),
    #[error("unsupported send reference: {0:?}")]
    UnsupportedSend(Reference),
    #[error("unsupported receive reference: {0:?}")]
    UnsupportedReceive(Reference),
}

// Traits
pub trait FetchConnector: Send + Sync {
    fn supports(&self, r: &Reference) -> bool;
    fn fetch<'a>(
        &'a self,
        fetch: &'a Fetch,
    ) -> Pin<Box<dyn futures::Future<Output = Result<ReadItemStream, ConnectorError>> + Send + 'a>>;
}

pub trait SendConnector: Send + Sync {
    fn supports(&self, r: &Reference) -> bool;
    fn send<'a>(
        &'a self,
        send: &'a SendRef,
    ) -> Pin<Box<dyn futures::Future<Output = Result<WriteItemStream, ConnectorError>> + Send + 'a>>;
}

pub trait ReceiveConnector: Send + Sync {
    fn supports(&self, r: &Reference) -> bool;
    fn receive<'a>(
        &'a self,
        recv: &'a Receive,
    ) -> Pin<Box<dyn futures::Future<Output = Result<ReadItemStream, ConnectorError>> + Send + 'a>>;
}

// Main router
pub struct Connector<T>
where
    T: Clone
        + StreamInterface
        + StreamReceiverInterface
        + StreamSenderInterface
        + Send
        + Sync
        + 'static,
{
    network: T,
    fetchers: Vec<Arc<dyn FetchConnector>>,
    senders: Vec<Arc<dyn SendConnector>>,
    receivers: Vec<Arc<dyn ReceiveConnector>>,
    _marker: std::marker::PhantomData<T>,
}

impl<T> Connector<T>
where
    T: Clone
        + StreamInterface
        + StreamReceiverInterface
        + StreamSenderInterface
        + Send
        + Sync
        + 'static,
{
    pub fn new(network: T) -> Self {
        let mut this = Self {
            network: network.clone(),
            fetchers: Vec::new(),
            senders: Vec::new(),
            receivers: Vec::new(),
            _marker: Default::default(),
        };
        // Register built-ins
        this.fetchers.push(Arc::new(HttpHfFetcher));
        let peer = PeerStreamConnector {
            network: network.clone(),
        };
        this.senders.push(Arc::new(peer.clone()));
        this.receivers.push(Arc::new(peer));
        this
    }

    pub fn with_fetcher<C>(mut self, f: C) -> Self
    where
        C: FetchConnector + 'static,
    {
        self.fetchers.push(Arc::new(f));
        self
    }
    pub fn with_sender<C>(mut self, s: C) -> Self
    where
        C: SendConnector + 'static,
    {
        self.senders.push(Arc::new(s));
        self
    }
    pub fn with_receiver<C>(mut self, r: C) -> Self
    where
        C: ReceiveConnector + 'static,
    {
        self.receivers.push(Arc::new(r));
        self
    }

    pub async fn fetch(&self, fetch: Fetch) -> Result<ReadItemStream, ConnectorError> {
        let r = fetch.as_ref().clone();
        for f in &self.fetchers {
            if f.supports(&r) {
                return f.fetch(&fetch).await;
            }
        }
        Err(ConnectorError::UnsupportedFetch(r))
    }

    pub async fn send(&self, send: SendRef) -> Result<WriteItemStream, ConnectorError> {
        let r = send.as_ref().clone();
        for s in &self.senders {
            if s.supports(&r) {
                return s.send(&send).await;
            }
        }
        Err(ConnectorError::UnsupportedSend(r))
    }

    pub async fn receive(&self, recv: Receive) -> Result<ReadItemStream, ConnectorError> {
        let r = recv.as_ref().clone();
        for rc in &self.receivers {
            if rc.supports(&r) {
                return rc.receive(&recv).await;
            }
        }
        Err(ConnectorError::UnsupportedReceive(r))
    }

    // One-item helpers removed; Connector exposes streaming APIs only.
}

impl<T> Clone for Connector<T>
where
    T: Clone
        + StreamInterface
        + StreamReceiverInterface
        + StreamSenderInterface
        + Send
        + Sync
        + 'static,
{
    fn clone(&self) -> Self {
        Self {
            network: self.network.clone(),
            fetchers: self.fetchers.clone(),
            senders: self.senders.clone(),
            receivers: self.receivers.clone(),
            _marker: Default::default(),
        }
    }
}

// --------------------
// Built-in connectors
// --------------------

#[derive(Clone)]
struct HttpHfFetcher;

impl FetchConnector for HttpHfFetcher {
    fn supports(&self, r: &Reference) -> bool {
        matches!(r, Reference::Uri { .. } | Reference::HuggingFace { .. })
    }

    fn fetch<'a>(
        &'a self,
        fetch: &'a Fetch,
    ) -> Pin<Box<dyn Future<Output = Result<ReadItemStream, ConnectorError>> + Send + 'a>> {
        Box::pin(async move {
            match fetch.as_ref() {
                Reference::Uri { value } => {
                    let client = reqwest::Client::new();
                    let response = client.get(value).send().await?;
                    if !response.status().is_success() {
                        return Err(ConnectorError::Http(reqwest::Error::from(
                            response.error_for_status().unwrap_err(),
                        )));
                    }
                    let byte_stream = response
                        .bytes_stream()
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e));
                    let tokio_reader = StreamReader::new(byte_stream);
                    let fut_reader = tokio_reader.compat();

                    let name = value.rsplit('/').next().unwrap_or("").to_string();
                    let item = ReadItem {
                        meta: ItemMeta { kind: "uri", name },
                        reader: Box::pin(fut_reader),
                    };
                    let s = futures::stream::once(async move { Ok(item) });
                    Ok(Box::pin(s) as ReadItemStream)
                }
                Reference::HuggingFace {
                    repository,
                    revision,
                    filenames,
                } => {
                    if filenames.is_empty() {
                        let s = futures::stream::empty::<Result<ReadItem, io::Error>>();
                        return Ok(Box::pin(s) as ReadItemStream);
                    }

                    // Build HF API and compute urls
                    let api = ApiBuilder::new().build()?;
                    let rev = revision.as_deref().unwrap_or("main");
                    let repo = api.repo(hf_hub::Repo::with_revision(
                        repository.clone(),
                        hf_hub::RepoType::Model,
                        rev.to_string(),
                    ));
                    let repo = std::sync::Arc::new(repo);

                    let stream = futures::stream::iter(filenames.clone()).then(move |filename| {
                        let repo = repo.clone();
                        async move {
                            // Download to cache (if needed) and get local path
                            let path = repo
                                .get(&filename)
                                .await
                                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                            // Open as tokio file and adapt to futures::io::AsyncRead
                            let file = tokio::fs::File::open(path)
                                .await
                                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                            let reader = file.compat();
                            Ok(ReadItem {
                                meta: ItemMeta {
                                    kind: "huggingface",
                                    name: filename,
                                },
                                reader: Box::pin(reader),
                            })
                        }
                    });
                    Ok(Box::pin(stream) as ReadItemStream)
                }
                _ => Err(ConnectorError::UnsupportedFetch(fetch.as_ref().clone())),
            }
        })
    }
}

#[derive(Clone)]
struct PeerStreamConnector<T>
where
    T: Clone
        + StreamInterface
        + StreamReceiverInterface
        + StreamSenderInterface
        + Send
        + Sync
        + 'static,
{
    network: T,
}

impl<T> SendConnector for PeerStreamConnector<T>
where
    T: Clone
        + StreamInterface
        + StreamReceiverInterface
        + StreamSenderInterface
        + Send
        + Sync
        + 'static,
{
    fn supports(&self, r: &Reference) -> bool {
        matches!(r, Reference::Peers { .. })
    }

    fn send<'a>(
        &'a self,
        send: &'a SendRef,
    ) -> Pin<Box<dyn futures::Future<Output = Result<WriteItemStream, ConnectorError>> + Send + 'a>>
    {
        Box::pin(async move {
            match send.as_ref() {
                Reference::Peers { peers, strategy } => match strategy {
                    SelectionStrategy::All => {
                        let network = self.network.clone();
                        let it = futures::stream::iter(peers.clone()).then(move |peer| {
                            let network = network.clone();
                            async move {
                                match network.stream(peer).await {
                                    Ok(stream) => Ok(WriteItem {
                                        meta: ItemMeta {
                                            kind: "peer",
                                            name: peer.to_string(),
                                        },
                                        writer: Box::pin(stream),
                                    }),
                                    Err(e) => {
                                        tracing::error!(
                                            "Failed to open stream to peer {}: {}",
                                            peer,
                                            e
                                        );
                                        Err(ConnectorError::OpenStream(e))
                                    }
                                }
                            }
                        });
                        Ok(Box::pin(it) as WriteItemStream)
                    }
                    SelectionStrategy::One | SelectionStrategy::Random => {
                        let peer = peers.get(0).copied().ok_or_else(|| {
                            io::Error::new(io::ErrorKind::Other, "no peers provided")
                        })?;
                        match self.network.stream(peer).await {
                            Ok(stream) => {
                                let item = WriteItem {
                                    meta: ItemMeta {
                                        kind: "peer",
                                        name: peer.to_string(),
                                    },
                                    writer: Box::pin(stream),
                                };
                                let s = futures::stream::once(async move { Ok(item) });
                                Ok(Box::pin(s) as WriteItemStream)
                            }
                            Err(e) => {
                                tracing::error!("Failed to open stream to peer {}: {}", peer, e);
                                Err(ConnectorError::OpenStream(e))
                            }
                        }
                    }
                },
                _ => Err(ConnectorError::UnsupportedSend(send.as_ref().clone())),
            }
        })
    }
}

impl<T> ReceiveConnector for PeerStreamConnector<T>
where
    T: Clone
        + StreamInterface
        + StreamReceiverInterface
        + StreamSenderInterface
        + Send
        + Sync
        + 'static,
{
    fn supports(&self, r: &Reference) -> bool {
        matches!(r, Reference::Peers { .. })
    }

    fn receive<'a>(
        &'a self,
        recv: &'a Receive,
    ) -> Pin<Box<dyn futures::Future<Output = Result<ReadItemStream, ConnectorError>> + Send + 'a>>
    {
        Box::pin(async move {
            match recv.as_ref() {
                Reference::Peers {
                    peers,
                    strategy: SelectionStrategy::All,
                } => {
                    let allow: Vec<PeerId> = peers.clone();
                    let incoming = self.network.streams()?;
                    let stream = incoming.filter_map(move |(peer, s)| {
                        tracing::info!("Recieving data from peer, {:?}", peer);
                        let allowed = allow.clone();
                        async move {
                            if allowed.is_empty() || allowed.iter().any(|p| p == &peer) {
                                let item = ReadItem {
                                    meta: ItemMeta {
                                        kind: "peer",
                                        name: peer.to_string(),
                                    },
                                    reader: Box::pin(s),
                                };
                                Some(Ok(item))
                            } else {
                                None
                            }
                        }
                    });
                    Ok(Box::pin(stream) as ReadItemStream)
                }
                _ => Err(ConnectorError::UnsupportedReceive(recv.as_ref().clone())),
            }
        })
    }
}

// (no dead utilities)
