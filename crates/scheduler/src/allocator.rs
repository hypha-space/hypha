use std::{
    future::Future,
    pin::{Pin, pin},
    task::Poll,
    time::{Duration, SystemTime},
};

use futures_util::{Stream, StreamExt};
use hypha_messages::{
    Request, Response, WorkerSpec, request_worker,
    worker_offer::{self, Request as WorkerOfferRequest},
};
use hypha_network::{gossipsub::GossipsubInterface, request_response::RequestResponseInterfaceExt};
use libp2p::PeerId;
use pin_project::pin_project;
use thiserror::Error;
use tokio::{sync::mpsc, time::Sleep};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::{network::Network, worker::Worker};

const WORKER_TOPIC: &str = "hypha/worker";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum AllocatorError {
    #[error("Failed to broadcast worker request")]
    BroadcastFailed(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("No workers available")]
    NoWorkersAvailable,
    #[error("No offers received")]
    NoOffersReceived,
    #[error("Request timeout")]
    Timeout,
    #[error("Failed to create lease")]
    LeaseFailed(#[from] hypha_leases::LedgerError),
}

/// Trait for different worker allocation strategies
pub trait Allocator: Send + Sync {
    /// Request a worker matching the given specification offered for no more than the given price
    fn request(
        &self,
        spec: WorkerSpec,
        price: f64,
        deadline: Option<Duration>,
    ) -> impl Future<Output = Result<Worker, AllocatorError>> + Send;
}

/// Greedy allocator that selects the first worker offering the lowest price
pub struct GreedyWorkerAllocator {
    network: Network,
}

impl GreedyWorkerAllocator {
    pub fn new(network: Network) -> Self {
        Self { network }
    }
}

impl Allocator for GreedyWorkerAllocator {
    async fn request(
        &self,
        spec: WorkerSpec,
        bid: f64,
        deadline: Option<Duration>,
    ) -> Result<Worker, AllocatorError> {
        let id = Uuid::new_v4();
        let deadline = deadline.unwrap_or(DEFAULT_TIMEOUT);

        tracing::info!(
        request_id = %id,
        deadline = ?deadline,
            "Requesting worker"
        );

        // Set up channel for offers
        let (tx, rx) = mpsc::channel(100);

        // NOTE: Set up a handler to receive and ack offers
        let offer_handle = tokio::spawn(
            self.network
                .on(move |req: &Request| {
                    matches!(
                        req,
                        Request::WorkerOffer(
                        worker_offer::Request { request_id, .. }
                    ) if request_id == &id
                    )
                })
                .into_stream()
                .await
                .map_err(|e| AllocatorError::BroadcastFailed(Box::new(e)))?
                .respond_with_concurrent(None, move |request| {
                    let tx = tx.clone();
                    async move {
                        if let (peer_id, Request::WorkerOffer(offer)) = request {
                            let _ = tx.send((peer_id, offer)).await;
                        }

                        Response::WorkerOffer(worker_offer::Response {})
                    }
                }),
        );

        // NOTE: Broadcast the worker request using gossipsub
        let mut message = Vec::new();
        ciborium::into_writer(
            &request_worker::Request {
                id,
                spec: spec.clone(),
                timeout: SystemTime::now() + deadline,
                bid,
            },
            &mut message,
        )
        .expect("serialized worker request");

        self.network
            .publish(WORKER_TOPIC, message)
            .await
            .map_err(|e| AllocatorError::BroadcastFailed(Box::new(e)))?;

        // NOTE: Aggregate offers and select the best one using the GreedyOfferAggregator
        let mut offer_aggregator = pin!(GreedyOfferAggregator::new(
            ReceiverStream::new(rx),
            id,
            deadline,
            None, // TODO: configure max offers
        ));

        let offer = offer_aggregator.next().await;

        // NOTE: Always abort the response handler task to prevent resource leaks
        // The task should be cancelled regardless of whether we found an offer or not
        offer_handle.abort();

        match offer {
            Some((peer_id, offer)) => {
                let worker =
                    Worker::create(offer.id, peer_id, spec, offer.price, self.network.clone())
                        .await;

                Ok(worker)
            }
            None => Err(AllocatorError::NoOffersReceived),
        }
    }
}

#[pin_project]
struct GreedyOfferAggregator<S> {
    #[pin]
    stream: S,
    #[pin]
    deadline: Sleep,
    best_offer: Option<(PeerId, WorkerOfferRequest)>,
    max_offers: Option<usize>,
    offers_received: usize,
}

impl<S> GreedyOfferAggregator<S> {
    pub fn new(
        stream: S,
        _request_id: Uuid,
        deadline: Duration,
        max_offers: Option<usize>,
    ) -> Self {
        GreedyOfferAggregator {
            stream,
            deadline: tokio::time::sleep(deadline),
            best_offer: None,
            max_offers,
            offers_received: 0,
        }
    }
}

impl<S> Stream for GreedyOfferAggregator<S>
where
    S: Stream<Item = (PeerId, WorkerOfferRequest)> + Unpin,
{
    type Item = (PeerId, WorkerOfferRequest);

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        // First check if we've reached max offers limit
        if let Some(max) = this.max_offers
            && *this.offers_received >= *max
        {
            tracing::debug!("Reached maximum offer limit");
            return Poll::Ready(this.best_offer.take());
        }

        // Poll the deadline timer
        if this.deadline.as_mut().poll(cx).is_ready() {
            tracing::debug!("Deadline reached");
            return Poll::Ready(this.best_offer.take());
        }

        // Poll for new offers
        loop {
            match this.stream.as_mut().poll_next(cx) {
                Poll::Ready(Some((peer_id, offer))) => {
                    *this.offers_received += 1;

                    // Update best offer if this one is better
                    let should_update_timer = match this.best_offer {
                        Some((_, current_best)) if current_best.price <= offer.price => {
                            // Keep current best
                            false
                        }
                        _ => {
                            tracing::debug!(
                                peer_id = %peer_id,
                                price = offer.price,
                                "New best offer"
                            );
                            let _offer_timeout = offer.timeout;
                            *this.best_offer = Some((peer_id, offer));
                            true
                        }
                    };

                    // Update deadline timer if new best offer expires sooner
                    if should_update_timer && let Some((_, offer)) = &this.best_offer {
                        let now = SystemTime::now();
                        let expiry_buffer = Duration::from_millis(100);

                        // Calculate time until offer expires
                        if let Ok(time_until_expiry) = offer.timeout.duration_since(now) {
                            let duration_until_expiry = if time_until_expiry > expiry_buffer {
                                time_until_expiry - expiry_buffer
                            } else {
                                Duration::from_millis(0)
                            };

                            // Reset the timer to the earlier of current deadline or offer expiry
                            this.deadline
                                .as_mut()
                                .reset(tokio::time::Instant::now() + duration_until_expiry);
                        }
                    }

                    // Re-check termination conditions after processing this offer
                    continue;
                }
                Poll::Ready(None) => {
                    // Stream ended, return what we have
                    return Poll::Ready(this.best_offer.take());
                }
                Poll::Pending => {
                    // No more offers available right now
                    return Poll::Pending;
                }
            }
        }
    }
}
