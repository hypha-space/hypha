//! Request-Response Protocol
//!
//! This module provides a request-response protocol abstraction
//! for libp2p networks with a fluent API for handling incoming requests.
//!
//! # Examples
//!
//! These examples show how to use the request-response system with the types from your application.
//!
//! ## Setting up a Request Handler
//!
//! ```no_run
//! use hypha_network::{request_response::RequestResponseInterfaceExt, request_response::RequestResponseInterface};
//! use libp2p::request_response::cbor::codec::Codec;
//! use serde::{Deserialize, Serialize};
//! use futures_util::StreamExt;
//!
//! #[derive(Serialize, Deserialize, Debug, Clone)]
//! enum Request {
//!     Greetings(String),
//! }
//!
//! #[derive(Serialize, Deserialize, Debug, Clone)]
//! enum Response {
//!     Hello(),
//! }
//!
//! type MyCodec = Codec<Request, Response>;
//!
//! async fn handle_greetings_request(req: Request) -> Response {
//!     match req {
//!         Request::Greetings(message) => {
//!             // Do some work here...
//!             Response::Hello()
//!         }
//!     }
//! }
//!
//! # async fn example<N>(network: N) -> Result<(), Box<dyn std::error::Error>>
//! # where
//! #     N: RequestResponseInterface<MyCodec> + Clone + Send + Sync + 'static,
//! # {
//! // Create a handler that matches greetings requests
//! let greetings_handler = network
//!     .on(|req: &Request| matches!(req, Request::Greetings(_)))
//!     .buffer_size(64)
//!     .into_stream()
//!     .await?
//!     .respond_with_concurrent(Some(2), |(_, req)| handle_greetings_request(req));
//!
//! // Run the handler
//! greetings_handler.await;
//! # Ok(())
//! # }
//! ```
//!
//! ## Making Requests
//!
//! ```no_run
//! use hypha_network::{request_response::RequestResponseInterface};
//! use libp2p::{PeerId, request_response::cbor::codec::Codec};
//! use serde::{Deserialize, Serialize};
//! use std::str::FromStr;
//!
//! # #[derive(Serialize, Deserialize, Debug, Clone)]
//! # enum Request { Greetings(String) }
//! # #[derive(Serialize, Deserialize, Debug, Clone)]
//! # enum Response { Hello() }
//! # type MyCodec = Codec<Request, Response>;
//! # async fn example<N>(network: N) -> Result<(), Box<dyn std::error::Error>>
//! # where
//! #     N: RequestResponseInterface<MyCodec>,
//! # {
//! // Make a request to a specific peer
//! let peer_id = "12D3KooWGrfVU2HH5XBzjdanYRyW2RhPGp3R1F2nZQJ4i4JmWd6k".parse::<PeerId>()?;
//! let request = Request::Greetings("Hello, world!".to_string());
//!
//! let response = network.request(peer_id, request).await?;
//! match response {
//!     Response::Hello() => println!("Received Hello!"),
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Pattern Matching
//!
//! The fluent API supports various pattern matching approaches:
//!
//! ```no_run
//! # use hypha_network::{request_response::RequestResponseInterfaceExt, request_response::RequestResponseInterface};
//! # use libp2p::request_response::cbor::codec::Codec;
//! # use serde::{Deserialize, Serialize};
//! # #[derive(Serialize, Deserialize, Debug, Clone)]
//! # enum Request { Work(String) }
//! # #[derive(Serialize, Deserialize, Debug, Clone)]
//! # enum Response { Done() }
//! # type MyCodec = Codec<Request, Response>;
//! # async fn example<N>(network: N) -> Result<(), Box<dyn std::error::Error>>
//! # where
//! #     N: RequestResponseInterface<MyCodec> + Clone + Send + Sync + 'static,
//! # {
//! // Simple closure pattern
//! let handler1 = network
//!     .on(|req: &Request| matches!(req, Request::Work(_)))
//!     .into_stream()
//!     .await?;
//!
//! // More complex pattern matching
//! let handler2 = network
//!     .on(|req: &Request| {
//!         match req {
//!             Request::Work(msg) => msg.starts_with("priority:"),
//!         }
//!     })
//!     .buffer_size(128)
//!     .into_stream()
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Concurrent Request Processing
//!
//! ```no_run
//! # use hypha_network::{request_response::RequestResponseInterfaceExt, request_response::RequestResponseInterface};
//! # use libp2p::request_response::cbor::codec::Codec;
//! # use serde::{Deserialize, Serialize};
//! # #[derive(Serialize, Deserialize, Debug, Clone)]
//! # enum Request { Work(String) }
//! # #[derive(Serialize, Deserialize, Debug, Clone)]
//! # enum Response { Done() }
//! # type MyCodec = Codec<Request, Response>;
//! # async fn example<N>(network: N) -> Result<(), Box<dyn std::error::Error>>
//! # where
//! #     N: RequestResponseInterface<MyCodec> + Clone + Send + Sync + 'static,
//! # {
//! let handler = network
//!     .on(|req: &Request| matches!(req, Request::Work(_)))
//!     .into_stream()
//!     .await?
//!     .respond_with_concurrent(
//!         Some(4), // Process up to 4 requests concurrently
//!         |(_, req)| async move {
//!             // Simulate some async work
//!             tokio::time::sleep(std::time::Duration::from_millis(100)).await;
//!
//!             match req {
//!                 Request::Work(msg) => {
//!                     println!("Processed: {}", msg);
//!                     Response::Done()
//!                 }
//!             }
//!         }
//!     );
//!
//! handler.await;
//! # Ok(())
//! # }
//! ```
use std::{
    collections::HashMap,
    fmt::Display,
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicU64, Ordering},
    task::{Context, Poll},
};

use futures_util::{Stream, StreamExt};
use libp2p::{PeerId, request_response, swarm::NetworkBehaviour};
use pin_project::{pin_project, pinned_drop};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;

use crate::swarm::SwarmDriver;

/// Unique identifier for request handlers
///
/// Each handler gets a unique ID when created, which is used for
/// registration and cleanup operations.
///
/// # Examples
///
/// ```
/// # use hypha_network::request_response::HandlerId;
/// let id1 = HandlerId::new();
/// let id2 = HandlerId::new();
/// assert_ne!(id1, id2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HandlerId(u64);

impl HandlerId {
    /// Create a new unique handler ID
    ///
    /// # Examples
    ///
    /// ```
    /// # use hypha_network::request_response::HandlerId;
    /// let id = HandlerId::new();
    /// println!("Handler ID: {}", id);
    /// ```
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for HandlerId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for HandlerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Handler({})", self.0)
    }
}

/// Map of outbound requests.
///
/// Tracks requests to another peer created from an interface action that haven't been added to the swarm yet.
pub type OutboundRequests<TCodec> = HashMap<
    request_response::OutboundRequestId,
    oneshot::Sender<Result<<TCodec as request_response::Codec>::Response, RequestResponseError>>,
>;

/// Map of outbound responses.
///
/// Tracks responses to another peer created from an interface action that haven't been added to the swarm yet.
pub type OutboundResponses =
    HashMap<request_response::InboundRequestId, oneshot::Sender<Result<(), RequestResponseError>>>;

/// Map of inbound requests.
///
/// Tracks requests from another peer on the swarm, not yet sent to a handler.
pub struct InboundRequest<TCodec>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
{
    /// Unique identifier for the request.
    pub request_id: request_response::InboundRequestId,
    /// Channel to send the response to.
    pub channel: request_response::ResponseChannel<<TCodec as request_response::Codec>::Response>,
    /// Peer ID of the sender.
    pub peer_id: PeerId,
    /// Request messages.
    pub request: <TCodec as request_response::Codec>::Request,
}

type InboundRequestSender<TCodec> =
    mpsc::Sender<Result<InboundRequest<TCodec>, RequestResponseError>>;

type RequestMatcher<TCodec> =
    Box<dyn Fn(&<TCodec as request_response::Codec>::Request) -> bool + Send + Sync>;

/// Trait for converting patterns into request matchers
///
/// This trait allows various types to be used as patterns for matching incoming requests.
pub trait Pattern<TCodec>
where
    TCodec: request_response::Codec,
{
    /// Convert the pattern into a request matcher.
    fn into_matcher(self) -> RequestMatcher<TCodec>;
}

impl<TCodec, F> Pattern<TCodec> for F
where
    TCodec: request_response::Codec,
    F: Fn(&<TCodec as request_response::Codec>::Request) -> bool + Send + Sync + 'static,
{
    fn into_matcher(self) -> RequestMatcher<TCodec> {
        Box::new(self)
    }
}

/// Error type for request-response operations
#[derive(Debug, thiserror::Error)]
pub enum RequestResponseError {
    /// Outbound request failed
    #[error("Outbound request failed: {0}")]
    Request(#[from] request_response::OutboundFailure),

    /// Inbound response failed
    #[error("Inbound response failed: {0}")]
    Response(#[from] request_response::InboundFailure),

    /// Channel error
    #[error("Channel error: {0}")]
    ChannelError(String),

    /// No handler registered for request
    #[error("No handler registered for request")]
    NoHandler,

    /// Other error
    #[error("{0}")]
    Other(String),
}

/// Builder for fluent request handler API
///
/// This builder provides a fluent interface for registering request handlers.
/// Start by calling `.on()` with a pattern via `RequestResponseInterfaceExt`,
/// then chain methods to configure the handler.
///
/// # Examples
///
/// ```no_run
/// # use hypha_network::request_response::*;
/// # use serde::{Serialize, Deserialize};
/// # #[derive(Serialize, Deserialize, Clone)]
/// # enum MyRequest { TypeA }
/// # #[derive(Serialize, Deserialize, Clone)]
/// # struct MyResponse { status: String }
/// # type MyCodec = libp2p::request_response::cbor::codec::Codec<MyRequest, MyResponse>;
/// # async fn example(interface: impl RequestResponseInterface<MyCodec> + Clone + Send + Sync + 'static) -> Result<(), RequestResponseError> {
/// let handler_stream = interface
///     .on(|req: &MyRequest| matches!(req, MyRequest::TypeA))
///     .buffer_size(64)
///     .into_stream()
///     .await?;
/// # Ok(())
/// # }
/// ```
#[must_use = "HandlerBuilder must be converted to a stream to handle requests"]
pub struct HandlerBuilder<'a, TCodec, TInterface>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
    TInterface: RequestResponseInterface<TCodec> + Clone + Send + Sync + 'static,
{
    interface: &'a TInterface,
    matcher: RequestMatcher<TCodec>,
    buffer_size: usize,
}

impl<TCodec, TInterface> HandlerBuilder<'_, TCodec, TInterface>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
    TInterface: RequestResponseInterface<TCodec> + Clone + Send + Sync + 'static,
{
    /// Set the buffer size for the handler stream
    ///
    /// The buffer size determines how many pending requests can be queued
    /// for this handler before backpressure is applied.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use hypha_network::request_response::*;
    /// # use serde::{Serialize, Deserialize};
    /// # #[derive(Serialize, Deserialize, Clone)]
    /// # enum MyRequest { TypeA }
    /// # type MyCodec = libp2p::request_response::cbor::codec::Codec<MyRequest, String>;
    /// # async fn example(network: impl RequestResponseInterface<MyCodec> + Clone + Send + Sync + 'static) -> Result<(), RequestResponseError> {
    /// let handler = network
    ///     .on(|_: &MyRequest| true)
    ///     .buffer_size(128)  // Allow 128 pending requests
    ///     .into_stream()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size;
        self
    }

    /// Create a stream for handling matching requests
    pub async fn into_stream(
        self,
    ) -> Result<HandlerStream<TCodec, TInterface>, RequestResponseError> {
        let (tx, rx) = mpsc::channel(self.buffer_size);
        let id = HandlerId::new();

        self.interface
            .send(RequestResponseAction::RegisterHandler(id, self.matcher, tx))
            .await;

        let interface = self.interface.clone();
        Ok(HandlerStream {
            id,
            stream: ReceiverStream::new(rx),
            interface,
        })
    }
}

/// A Stream for handling matching requests
#[pin_project(PinnedDrop)]
pub struct HandlerStream<TCodec, TInterface>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
    TInterface: RequestResponseInterface<TCodec>,
{
    id: HandlerId,
    #[pin]
    stream: ReceiverStream<Result<InboundRequest<TCodec>, RequestResponseError>>,
    interface: TInterface,
}

impl<TCodec, TInterface> HandlerStream<TCodec, TInterface>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
    TInterface: RequestResponseInterface<TCodec>,
{
    /// Process requests with concurrent execution
    pub fn respond_with_concurrent<F, Fut>(
        self,
        max_concurrent: Option<usize>,
        handler: F,
    ) -> impl Future<Output = ()>
    where
        TInterface: Clone + Send + Sync + 'static,
        F: Fn((PeerId, <TCodec as request_response::Codec>::Request)) -> Fut
            + Clone
            + Send
            + Sync
            + 'static,
        Fut: Future<Output = <TCodec as request_response::Codec>::Response> + Send + 'static,
    {
        let interface = self.interface.clone();

        let max_concurrent = max_concurrent.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });

        self.for_each_concurrent(max_concurrent, move |result| {
            let interface = interface.clone();
            let handler = handler.clone();

            async move {
                match result {
                    Ok(inbound_request) => {
                        let response =
                            handler((inbound_request.peer_id, inbound_request.request)).await;

                        if let Err(e) = interface
                            .respond(
                                inbound_request.request_id,
                                inbound_request.channel,
                                response,
                            )
                            .await
                        {
                            tracing::error!(error=?e, "Error sending response");
                        }
                    }
                    Err(e) => {
                        tracing::error!(error=?e, "Error processing inbound request");
                    }
                }
            }
        })
    }
}

impl<TCodec, TInterface> Stream for HandlerStream<TCodec, TInterface>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
    TInterface: RequestResponseInterface<TCodec>,
{
    type Item = Result<InboundRequest<TCodec>, RequestResponseError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        tracing::trace!("Polling HandlerStream");
        let this = self.project();
        this.stream.poll_next(cx)
    }
}

#[pinned_drop]
impl<TCodec, TInterface> PinnedDrop for HandlerStream<TCodec, TInterface>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
    TInterface: RequestResponseInterface<TCodec>,
{
    fn drop(self: Pin<&mut Self>) {
        let this = self.project();
        if let Err(error) = this
            .interface
            .try_send(RequestResponseAction::UnregisterHandler(*this.id))
        {
            tracing::error!( error = ?error, "Failed to unregister handler");
        }
    }
}

/// Wraps a request matcher with a request sender
pub struct RequestHandler<TCodec>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
{
    id: HandlerId,
    matcher: RequestMatcher<TCodec>,
    sender: InboundRequestSender<TCodec>,
}

/// Trait for network behaviours that include request-response functionality.
///
/// This trait provides access to the request-response behaviour within a composite
/// network behaviour. It's used by other traits that need to interact with
/// the request-response protocol.
pub trait RequestResponseBehaviour<TCodec>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
{
    /// Returns a mutable reference to the request-response behaviour.
    fn request_response(&mut self) -> &mut request_response::Behaviour<TCodec>;
}

/// Actions that can be sent to a request-response driver for processing.
pub enum RequestResponseAction<TCodec>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
{
    /// A request to be sent to a peer.
    OutboundRequest(
        PeerId,
        <TCodec as request_response::Codec>::Request,
        oneshot::Sender<
            Result<<TCodec as request_response::Codec>::Response, RequestResponseError>,
        >,
    ),

    /// A response to a request sent from a peer to be sent back to that peer.
    OutboundResponse(
        request_response::InboundRequestId,
        request_response::ResponseChannel<<TCodec as request_response::Codec>::Response>,
        <TCodec as request_response::Codec>::Response,
        oneshot::Sender<Result<(), RequestResponseError>>,
    ),

    /// Register a handler for requests of a specific type.
    RegisterHandler(
        HandlerId,
        RequestMatcher<TCodec>,
        InboundRequestSender<TCodec>,
    ),

    /// Unregister a handler.
    UnregisterHandler(HandlerId),
}

/// Trait for handling request-response operations within a swarm driver.
pub trait RequestResponseDriver<TBehaviour, TCodec>: SwarmDriver<TBehaviour> + Send
where
    TBehaviour: NetworkBehaviour + RequestResponseBehaviour<TCodec>,
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
{
    /// Returns a mutable reference to the outbound requests.
    ///
    /// This is used to manage incoming request from the driver
    /// that haven't been added to the swarm yet.
    fn outbound_requests(&mut self) -> &mut OutboundRequests<TCodec>;

    /// Returns a mutable reference to the outbound responses.
    ///
    /// This is used to manage outgoing responses from the driver
    /// that haven't been added to the swarm yet.
    fn outbound_responses(&mut self) -> &mut OutboundResponses;

    /// Returns a mutable reference to the request handlers.
    ///
    /// Incoming requests will be handled by the request handlers.
    fn request_handlers(&mut self) -> &mut Vec<RequestHandler<TCodec>>;

    /// Process a request-response action from the interface.
    fn process_request_response_action(
        &mut self,
        action: RequestResponseAction<TCodec>,
    ) -> impl Future<Output = ()> + Send {
        async move {
            match action {
                RequestResponseAction::OutboundRequest(peer_id, request, tx) => {
                    let id = self
                        .swarm()
                        .behaviour_mut()
                        .request_response()
                        .send_request(&peer_id, request);
                    self.outbound_requests().insert(id, tx);
                }
                RequestResponseAction::OutboundResponse(_id, channel, response, tx) => {
                    let result = self
                        .swarm()
                        .behaviour_mut()
                        .request_response()
                        .send_response(channel, response)
                        .map_err(|_| {
                            RequestResponseError::Other("Failed to send response".to_string())
                        });

                    if tx.send(result).is_err() {
                        tracing::debug!("Failed to send response result back to request handler");
                    }
                }
                RequestResponseAction::RegisterHandler(id, matcher, sender) => {
                    let handler = RequestHandler {
                        id,
                        matcher,
                        sender,
                    };
                    self.request_handlers().push(handler);
                }
                RequestResponseAction::UnregisterHandler(id) => {
                    self.request_handlers().retain(|handler| handler.id != id);
                }
            }
        }
    }

    /// Process a request-response event from the swarm.
    fn process_request_response_event(
        &mut self,
        event: request_response::Event<
            <TCodec as request_response::Codec>::Request,
            <TCodec as request_response::Codec>::Response,
        >,
    ) -> impl Future<Output = ()> + Send {
        async move {
            use request_response::Event;
            match event {
                Event::InboundFailure {
                    peer,
                    request_id,
                    error,
                    ..
                } => {
                    tracing::error!(
                        peer = %peer,
                        request_id = ?request_id,
                        error = ?error,
                        "Inbound request-response failure"
                    );
                }
                Event::Message { peer, message, .. } => match message {
                    request_response::Message::Request {
                        request_id,
                        request,
                        channel,
                    } => {
                        tracing::trace!(
                            peer = %peer,
                            request_id = ?request_id,
                            "Received inbound request"
                        );

                        let handlers = self.request_handlers();

                        // NOTE: Only the first matching handler is used as a request can only ever be handled (responded) by one handler.
                        let handler = handlers.iter().find(|h| (h.matcher)(&request));

                        if let Some(handler) = handler {
                            match handler
                                .sender
                                .send(Ok(InboundRequest {
                                    request_id,
                                    channel,
                                    peer_id: peer,
                                    request,
                                }))
                                .await
                            {
                                Ok(_) => {
                                    tracing::trace!(
                                        peer = %peer,
                                        request_id = ?request_id,
                                        handler_id = %handler.id,
                                        "Successfully sent request to handler channel"
                                    );
                                }
                                Err(_) => {
                                    tracing::warn!(
                                        peer = %peer,
                                        handler_id = %handler.id,
                                        "Handler channel closed, request dropped"
                                    );
                                }
                            }
                        } else {
                            tracing::warn!(
                                peer = %peer,
                                request_id = ?request_id,
                                "No handler registered, request dropped"
                            );
                        }
                    }
                    request_response::Message::Response {
                        request_id,
                        response,
                    } => {
                        if let Some(tx) = self.outbound_requests().remove(&request_id) {
                            if tx.send(Ok(response)).is_err() {
                                tracing::debug!(
                                    peer = %peer,
                                    request_id = ?request_id,
                                    "Response receiver dropped"
                                );
                            }
                        } else {
                            tracing::warn!(
                                peer = %peer,
                                request_id = ?request_id,
                                "Received response for unknown request"
                            );
                        }
                    }
                },
                Event::OutboundFailure {
                    peer,
                    request_id,
                    error,
                    ..
                } => {
                    tracing::error!(
                        peer = %peer,
                        request_id = ?request_id,
                        error = ?error,
                        "Outbound request-response failure"
                    );

                    if let Some(tx) = self.outbound_requests().remove(&request_id)
                        && tx.send(Err(RequestResponseError::Request(error))).is_err()
                    {
                        tracing::debug!("Failed to send error to request handler");
                    }
                }
                Event::ResponseSent {
                    peer, request_id, ..
                } => {
                    tracing::trace!(
                        peer = %peer,
                        request_id = ?request_id,
                        "Response sent successfully"
                    );

                    if let Some(tx) = self.outbound_responses().remove(&request_id)
                        && tx.send(Ok(())).is_err()
                    {
                        tracing::debug!("Response confirmation receiver dropped");
                    }
                }
            }
        }
    }
}

/// Interface for sending request-response actions to a network driver.
pub trait RequestResponseInterface<TCodec>: Send + Sync
where
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
{
    /// Send a request-response action to the network driver.
    fn send(&self, action: RequestResponseAction<TCodec>) -> impl Future<Output = ()> + Send;

    /// Try to send a request-response action to the network driver synchronously.
    fn try_send(&self, action: RequestResponseAction<TCodec>) -> Result<(), RequestResponseError>;

    /// Send a request and await the response
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use hypha_network::request_response::*;
    /// # use libp2p::PeerId;
    /// # use serde::{Serialize, Deserialize};
    /// # #[derive(Serialize, Deserialize, Clone)]
    /// # struct MyRequest { data: String }
    /// # #[derive(Serialize, Deserialize, Clone)]
    /// # struct MyResponse { result: String }
    /// # type MyCodec = libp2p::request_response::cbor::codec::Codec<MyRequest, MyResponse>;
    /// # async fn example(network: impl RequestResponseInterface<MyCodec>, peer: PeerId) -> Result<(), RequestResponseError> {
    /// let request = MyRequest { data: "hello".to_string() };
    /// let response = network.request(peer, request).await?;
    /// println!("Got response: {}", response.result);
    /// # Ok(())
    /// # }
    /// ```
    fn request(
        &self,
        peer_id: PeerId,
        request: <TCodec as request_response::Codec>::Request,
    ) -> impl Future<
        Output = Result<<TCodec as request_response::Codec>::Response, RequestResponseError>,
    > + Send {
        async move {
            let (tx, rx) = oneshot::channel();
            self.send(RequestResponseAction::OutboundRequest(peer_id, request, tx))
                .await;
            rx.await.map_err(|_| {
                RequestResponseError::ChannelError(
                    "Response channel closed unexpectedly".to_string(),
                )
            })?
        }
    }

    /// Send a response to an inbound request
    ///
    /// This is typically called by request handlers to respond to incoming requests.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use hypha_network::request_response::*;
    /// # use serde::{Serialize, Deserialize};
    /// # #[derive(Serialize, Deserialize, Clone)]
    /// # struct MyResponse { result: String }
    /// # type MyCodec = libp2p::request_response::cbor::codec::Codec<String, MyResponse>;
    /// # async fn example(network: impl RequestResponseInterface<MyCodec>, request: InboundRequest<MyCodec>) -> Result<(), RequestResponseError> {
    /// let response = MyResponse { result: "processed".to_string() };
    /// network.respond(request.request_id, request.channel, response).await?;
    /// # Ok(())
    /// # }
    /// ```
    fn respond(
        &self,
        request_id: request_response::InboundRequestId,
        channel: request_response::ResponseChannel<<TCodec as request_response::Codec>::Response>,
        response: <TCodec as request_response::Codec>::Response,
    ) -> impl Future<Output = Result<(), RequestResponseError>> + Send {
        async move {
            let (tx, rx) = oneshot::channel();
            self.send(RequestResponseAction::OutboundResponse(
                request_id, channel, response, tx,
            ))
            .await;
            rx.await.map_err(|_| {
                RequestResponseError::ChannelError(
                    "Response confirmation channel closed".to_string(),
                )
            })?
        }
    }
}

/// Convenience trait to call typed on()/request() without creating a ProtocolHandle.
///
/// This enables `network.on::<P>(...)` and `network.request::<P>(...)` where `P: Protocol`.
pub trait RequestResponseInterfaceExt: Clone + Sized + Send + Sync + 'static {
    /// Create a handler builder for the protocol `P` using the given pattern.
    fn on<TCodec, Pat>(&self, pattern: Pat) -> HandlerBuilder<'_, TCodec, Self>
    where
        TCodec: request_response::Codec + Clone + Send + Sync + 'static,
        TCodec::Request: Clone + Send + 'static,
        Self: RequestResponseInterface<TCodec> + Clone + Send + Sync + 'static,
        Pat: Pattern<TCodec>,
    {
        HandlerBuilder {
            interface: self,
            matcher: pattern.into_matcher(),
            buffer_size: 32,
        }
    }

    /// Send a typed request for protocol `P`.
    fn request<TCodec>(
        &self,
        peer_id: PeerId,
        request: TCodec::Request,
    ) -> impl Future<Output = Result<TCodec::Response, RequestResponseError>> + Send
    where
        TCodec: request_response::Codec + Clone + Send + Sync + 'static,
        TCodec::Request: Clone + Send + 'static,
        Self: RequestResponseInterface<TCodec>,
    {
        <Self as RequestResponseInterface<TCodec>>::request(self, peer_id, request)
    }
}

impl<T> RequestResponseInterfaceExt for T where T: Clone + Sized + Send + Sync + 'static {}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use libp2p::request_response::cbor::codec::Codec;
    use mockall::mock;
    use proptest::prelude::*;
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    enum TestRequest {
        TypeA { data: String },
        TypeB { data: i32 },
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestResponse {
        status: String,
    }

    type TestCodec = Codec<TestRequest, TestResponse>;

    mock! {
        Interface {}

        impl Clone for Interface {
               fn clone(&self) -> Self;
        }

        impl RequestResponseInterface<TestCodec> for Interface {
            async fn send(&self, action: RequestResponseAction<TestCodec>);
            fn try_send(&self, action: RequestResponseAction<TestCodec>) -> Result<(), RequestResponseError>;
        }
    }

    proptest! {
        #[test]
        fn test_handler_id_uniqueness(len in 2usize..100usize) {
            let ids: Vec<HandlerId> = (0..len).map(|_| HandlerId::new()).collect();

            let unique: HashSet<_> = ids.iter().cloned().collect();
            prop_assert_eq!(unique.len(), ids.len(), "IDs must be unique");
        }

        #[test]
        fn test_handler_id_monotonicity(len in 2usize..100usize) {
            let ids: Vec<HandlerId> = (0..len).map(|_| HandlerId::new()).collect();

            for window in ids.windows(2) {
                prop_assert!(window[0].0 < window[1].0,
                    "HandlerId({}) should be < HandlerId({})", window[0].0, window[1].0);
            }
        }
    }

    #[test]
    fn test_pattern_matcher_from_closure() {
        fn create_pattern() -> impl Pattern<TestCodec> {
            |req: &TestRequest| matches!(req, TestRequest::TypeA { .. })
        }

        let pattern = create_pattern();
        let matcher = pattern.into_matcher();

        let type_a = TestRequest::TypeA {
            data: "test".to_string(),
        };
        let type_b = TestRequest::TypeB { data: 42 };

        assert!(matcher(&type_a));
        assert!(!matcher(&type_b));
    }

    mod handler_builder {
        use super::*;

        #[test]
        fn test_adjusts_buffer_size_with_buffer_size() {
            let mock = MockInterface::new();

            let builder = HandlerBuilder {
                interface: &mock,
                matcher: Box::new(|_| true),
                buffer_size: 32,
            };

            assert_eq!(builder.buffer_size(42).buffer_size, 42);
        }

        #[tokio::test]
        async fn test_registers_handler_with_into_stream() {
            let mut mock = MockInterface::new();

            // NOTE: The interface is cloned and passed to the handler stream via the handler builder to unregister handlers on drop.
            let mut cloned_mock = MockInterface::new();

            cloned_mock
                .expect_try_send()
                .withf(|action| matches!(action, RequestResponseAction::UnregisterHandler(_)))
                .returning(|_| Ok(()))
                .times(1);

            mock.expect_clone().return_once(move || cloned_mock);

            mock.expect_send()
                .withf(|action| matches!(action, RequestResponseAction::RegisterHandler { .. }))
                .return_const(())
                .times(1);

            // NOTE: The stream is dropped here, which will unregister the handler.
            let _ = crate::request_response::RequestResponseInterfaceExt::on(
                &mock,
                |req: &TestRequest| matches!(req, TestRequest::TypeA { .. }),
            )
            .into_stream()
            .await;
        }
    }

    mod handler_stream {
        use super::*;

        #[tokio::test]
        async fn test_unregister_handler_on_drop() {
            let mut mock = MockInterface::new();

            // NOTE: The interface is cloned and passed to the handler stream via the handler builder to unregister handlers on drop.
            let mut cloned_mock = MockInterface::new();

            cloned_mock
                .expect_try_send()
                .withf(|action| matches!(action, RequestResponseAction::UnregisterHandler(_)))
                .returning(|_| {
                    Err(RequestResponseError::Other(
                        "Failed to send action".to_string(),
                    ))
                })
                .times(1);

            mock.expect_clone().return_once(move || cloned_mock);

            mock.expect_send()
                .withf(|action| matches!(action, RequestResponseAction::RegisterHandler { .. }))
                .return_const(())
                .times(1);

            // NOTE: The stream is dropped here, which will unregister the handler.
            let _ = crate::request_response::RequestResponseInterfaceExt::on(
                &mock,
                |req: &TestRequest| matches!(req, TestRequest::TypeA { .. }),
            )
            .into_stream()
            .await;
        }

        #[tokio::test]
        async fn test_drop_handles_unregister_error() {
            let mut mock = MockInterface::new();

            // NOTE: The interface is cloned and passed to the handler stream via the handler builder to unregister handlers on drop.
            let mut cloned_mock = MockInterface::new();

            cloned_mock
                .expect_try_send()
                .withf(|action| matches!(action, RequestResponseAction::UnregisterHandler(_)))
                .returning(|_| {
                    Err(RequestResponseError::Other(
                        "Failed to send action".to_string(),
                    ))
                })
                .times(1);

            mock.expect_clone().return_once(move || cloned_mock);

            mock.expect_send()
                .withf(|action| matches!(action, RequestResponseAction::RegisterHandler { .. }))
                .return_const(())
                .times(1);

            // NOTE: The stream is dropped here, which will unregister the handler.
            let _ = crate::request_response::RequestResponseInterfaceExt::on(
                &mock,
                |req: &TestRequest| matches!(req, TestRequest::TypeA { .. }),
            )
            .into_stream()
            .await;
        }
    }
}
