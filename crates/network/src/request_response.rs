use std::{collections::HashMap, error::Error, fmt::Display};

use futures_util::future::BoxFuture;
use libp2p::{PeerId, request_response, swarm::NetworkBehaviour};
use tokio::sync::{OnceCell, mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;

use crate::swarm::SwarmDriver;

pub type OutboundRequests<TCodec> = HashMap<
    request_response::OutboundRequestId,
    oneshot::Sender<Result<<TCodec as request_response::Codec>::Response, RequestResponseError>>,
>;
pub type OutboundResponses =
    HashMap<request_response::InboundRequestId, oneshot::Sender<Result<(), RequestResponseError>>>;

pub struct InboundRequest<TCodec>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
{
    pub request_id: request_response::InboundRequestId,
    pub channel: request_response::ResponseChannel<<TCodec as request_response::Codec>::Response>,
    pub request: <TCodec as request_response::Codec>::Request,
}

pub type InboundRequestSender<TCodec> =
    mpsc::Sender<Result<InboundRequest<TCodec>, RequestResponseError>>;

pub type InboundRequestReciever<TCodec> =
    mpsc::Receiver<Result<InboundRequest<TCodec>, RequestResponseError>>;

pub trait RequestResponseBehaviour<TCodec>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
{
    fn request_response(&mut self) -> &mut request_response::Behaviour<TCodec>;
}

pub enum RequestResponseAction<TCodec>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
{
    OutboundRequest(
        PeerId,
        <TCodec as request_response::Codec>::Request,
        oneshot::Sender<
            Result<<TCodec as request_response::Codec>::Response, RequestResponseError>,
        >,
    ),
    OutboundResponse(
        request_response::InboundRequestId,
        request_response::ResponseChannel<<TCodec as request_response::Codec>::Response>,
        <TCodec as request_response::Codec>::Response,
        oneshot::Sender<Result<(), RequestResponseError>>,
    ),
    InboundRequestHandle(InboundRequestSender<TCodec>),
}

pub enum RequestResponseEvent<TCodec>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
{
    InboundRequest(
        request_response::InboundRequestId,
        request_response::ResponseChannel<<TCodec as request_response::Codec>::Response>,
        <TCodec as request_response::Codec>::Request,
    ),
}

#[derive(Debug)]
pub enum RequestResponseError {
    Request(request_response::OutboundFailure),
    Response(request_response::InboundFailure),
    Other(String),
}

impl Error for RequestResponseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RequestResponseError::Request(err) => Some(err),
            RequestResponseError::Response(err) => Some(err),
            RequestResponseError::Other(_) => None,
        }
    }
}

impl Display for RequestResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestResponseError::Request(err) => write!(f, "Request error: {}", err),
            RequestResponseError::Response(err) => write!(f, "Response error: {}", err),
            RequestResponseError::Other(msg) => write!(f, "Other error: {}", msg),
        }
    }
}

#[allow(async_fn_in_trait)]
pub trait RequestResponseDriver<TBehaviour, TCodec>: SwarmDriver<TBehaviour>
where
    TBehaviour: NetworkBehaviour + RequestResponseBehaviour<TCodec>,
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
{
    fn outbound_requests(&mut self) -> &mut OutboundRequests<TCodec>;

    fn outbound_responses(&mut self) -> &mut OutboundResponses;

    /// Provides mutable sender which is used by the driver to forward requests to the interface.
    // TODO: Reconsider `OnceCell` maybe there is a betteway.
    fn inbound_request_sender(&self) -> &OnceCell<InboundRequestSender<TCodec>>;

    async fn process_request_response_action(&mut self, action: RequestResponseAction<TCodec>) {
        match action {
            RequestResponseAction::OutboundRequest(peer_id, request, tx) => {
                let request_id = self
                    .swarm()
                    .behaviour_mut()
                    .request_response()
                    .send_request(&peer_id, request);
                self.outbound_requests().insert(request_id, tx);
            }
            RequestResponseAction::OutboundResponse(request_id, ch, response, tx) => {
                match self
                    .swarm()
                    .behaviour_mut()
                    .request_response()
                    .send_response(ch, response)
                {
                    Ok(()) => {
                        self.outbound_responses().insert(request_id, tx);
                    }
                    Err(_) => {
                        let _ = tx.send(Err(RequestResponseError::Other(
                            "Response channel closed".to_string(),
                        )));
                    }
                }
            }
            RequestResponseAction::InboundRequestHandle(tx) => {
                // Update inbound_request_sender
                if let Err(e) = self.inbound_request_sender().set(tx) {
                    tracing::error!(error=?e, "Inbound request sender already set");
                }
            }
        }
    }

    async fn process_request_response_event(
        &mut self,
        event: request_response::Event<
            <TCodec as request_response::Codec>::Request,
            <TCodec as request_response::Codec>::Response,
        >,
    ) {
        match event {
            request_response::Event::Message { message, .. } => match message {
                request_response::Message::Request {
                    request_id,
                    request,
                    channel,
                } => {
                    if let Some(sender) = self.inbound_request_sender().get() {
                        if let Err(e) = sender
                            .send(Ok(InboundRequest {
                                request_id,
                                channel,
                                request,
                            }))
                            .await
                        {
                            // TODO: Instead of just logging an error we may want to panic as this indicates a serious issue with the driver/interface setup.
                            tracing::error!(
                                error=?e,
                                "Failed to send inbound request to handler channel"
                            );
                        }
                    } else {
                        tracing::warn!("No handler channel available yet send inbound request");
                    }
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    if let Some(tx) = self.outbound_requests().remove(&request_id) {
                        let _ = tx.send(Ok(response));
                    } else {
                        tracing::warn!(
                            request_id =?request_id,
                            "No sender for request"
                        );
                    }
                }
            },
            request_response::Event::ResponseSent { request_id, .. } => {
                if let Some(tx) = self.outbound_responses().remove(&request_id) {
                    let _ = tx.send(Ok(()));
                } else {
                    tracing::warn!(request_id=?request_id, "No sender for request", );
                }
            }
            request_response::Event::InboundFailure {
                request_id, error, ..
            } => {
                if let Some(tx) = self.outbound_responses().remove(&request_id) {
                    let _ = tx.send(Err(RequestResponseError::Response(error)));
                } else {
                    tracing::warn!(request_id=?request_id, "No sender for request id", );
                }
            }
            request_response::Event::OutboundFailure {
                request_id, error, ..
            } => {
                if let Some(tx) = self.outbound_requests().remove(&request_id) {
                    let _ = tx.send(Err(RequestResponseError::Request(error)));
                } else {
                    tracing::warn!(request_id=?request_id,"No sender for request id");
                }
            }
        }
    }
}

#[allow(async_fn_in_trait)]
pub type RequestPredicate<TCodec> =
    Box<dyn Fn(&<TCodec as request_response::Codec>::Request) -> bool + Send + Sync>;
pub type RequestHandler<TCodec> = Box<
    dyn Fn(
            <TCodec as request_response::Codec>::Request,
        ) -> BoxFuture<'static, <TCodec as request_response::Codec>::Response>
        + Send
        + Sync,
>;

#[allow(async_fn_in_trait)]
pub trait RequestResponseInterface<TCodec>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
    <TCodec as request_response::Codec>::Request: Clone + Send + 'static,
    <TCodec as request_response::Codec>::Response: Send + 'static,
{
    // TODO: Send should return a result and fail when an error occurs
    async fn send(&self, action: RequestResponseAction<TCodec>);

    async fn request(
        &self,
        peer_id: PeerId,
        request: <TCodec as request_response::Codec>::Request,
    ) -> Result<<TCodec as request_response::Codec>::Response, RequestResponseError> {
        let (tx, rx) = oneshot::channel();
        self.send(RequestResponseAction::OutboundRequest(peer_id, request, tx))
            .await;
        rx.await.unwrap()
    }

    async fn respond(
        &self,
        request_id: request_response::InboundRequestId,
        ch: request_response::ResponseChannel<<TCodec as request_response::Codec>::Response>,
        response: <TCodec as request_response::Codec>::Response,
    ) -> Result<(), RequestResponseError> {
        let (tx, rx) = oneshot::channel();
        self.send(RequestResponseAction::OutboundResponse(
            request_id, ch, response, tx,
        ))
        .await;
        rx.await.unwrap()
    }

    async fn requests(
        &self,
    ) -> Result<
        ReceiverStream<Result<InboundRequest<TCodec>, RequestResponseError>>,
        RequestResponseError,
    > {
        // TODO: Determine correct size
        let (tx, rx) = mpsc::channel(5);

        // Create inbound request handle
        self.send(RequestResponseAction::InboundRequestHandle(tx))
            .await;

        Ok(ReceiverStream::new(rx))
    }
}
