use std::{collections::HashMap, error::Error, fmt::Display};

use libp2p::{PeerId, request_response, swarm::NetworkBehaviour};
use tokio::sync::oneshot;

use crate::swarm::SwarmDriver;

pub type OutboundRequests<TCodec> = HashMap<
    request_response::OutboundRequestId,
    oneshot::Sender<Result<<TCodec as request_response::Codec>::Response, RequestResponseError>>,
>;
pub type OutboundResponses =
    HashMap<request_response::InboundRequestId, oneshot::Sender<Result<(), RequestResponseError>>>;

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
{
    async fn send(&mut self, event: RequestResponseEvent<TCodec>);

    fn outbound_requests(&mut self) -> &mut OutboundRequests<TCodec>;

    fn outbound_responses(&mut self) -> &mut OutboundResponses;

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
                    self.send(RequestResponseEvent::InboundRequest(
                        request_id, channel, request,
                    ))
                    .await;
                }
                request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    if let Some(tx) = self.outbound_requests().remove(&request_id) {
                        let _ = tx.send(Ok(response));
                    } else {
                        tracing::warn!("No sender for request id: {:?}", request_id);
                    }
                }
            },
            request_response::Event::ResponseSent { request_id, .. } => {
                if let Some(tx) = self.outbound_responses().remove(&request_id) {
                    let _ = tx.send(Ok(()));
                } else {
                    tracing::warn!("No sender for request id: {:?}", request_id);
                }
            }
            request_response::Event::InboundFailure {
                request_id, error, ..
            } => {
                if let Some(tx) = self.outbound_responses().remove(&request_id) {
                    let _ = tx.send(Err(RequestResponseError::Response(error)));
                } else {
                    tracing::warn!("No sender for request id: {:?}", request_id);
                }
            }
            request_response::Event::OutboundFailure {
                request_id, error, ..
            } => {
                if let Some(tx) = self.outbound_requests().remove(&request_id) {
                    let _ = tx.send(Err(RequestResponseError::Request(error)));
                } else {
                    tracing::warn!("No sender for request id: {:?}", request_id);
                }
            }
        }
    }
}

#[allow(async_fn_in_trait)]
pub trait RequestResponseInterface<TCodec>
where
    TCodec: request_response::Codec + Clone + Send + 'static,
{
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

    async fn response(
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
}

#[cfg(test)]
mod tests {
    use mockall::mock;
    use serde::{Deserialize, Serialize};

    use crate::cbor_codec::Codec;

    use super::*;

    #[derive(Serialize, Deserialize, Debug)]
    struct Request {
        value: String,
    }

    #[derive(Serialize, Deserialize, Debug)]
    struct Response {
        value: String,
    }

    mock! {
        TestInterface {}

        impl RequestResponseInterface<Codec<Request, Response>> for TestInterface {
            async fn send(&self, action: RequestResponseAction<Codec<Request, Response>>);
        }
    }

    #[tokio::test]
    async fn test_requestresponse_interface_request() {
        let mut mock = MockTestInterface::new();

        mock.expect_send()
            .withf(|action| matches!(action, RequestResponseAction::OutboundRequest(_, _, _)))
            .times(1)
            .returning(move |action| match action {
                RequestResponseAction::OutboundRequest(_, Request { value }, tx) => {
                    if value == "test".to_string() {
                        let _ = tx.send(Ok(Response {
                            value: "test".to_string(),
                        }));
                    }
                }
                _ => {}
            });

        let response = mock
            .request(
                PeerId::random(),
                Request {
                    value: "test".to_string(),
                },
            )
            .await;

        assert!(response.is_ok());
        let response = response.unwrap();

        assert!(response.value == "test".to_string());
    }
}
