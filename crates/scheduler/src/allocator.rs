use std::{
    future::Future,
    pin::{Pin, pin},
    task::Poll,
    time::{Duration, SystemTime},
};

use futures_util::{Stream, StreamExt};
use hypha_messages::{
    WorkerSpec, api, request_worker,
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
    /// Request `num` workers matching the given specification offered for no more than the given price
    /// NOTE: API changed to support multi-worker allocation in one shot.
    fn request(
        &self,
        spec: WorkerSpec,
        price: f64,
        deadline: Option<Duration>,
        num: usize,
    ) -> impl Future<Output = Result<Vec<Worker>, AllocatorError>> + Send;
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
        num: usize,
    ) -> Result<Vec<Worker>, AllocatorError> {
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
                .on::<api::Codec, _>(move |req: &api::Request| matches!(req, api::Request::WorkerOffer(worker_offer::Request { request_id, .. }) if request_id == &id))
                .into_stream()
                .await
                .map_err(|e| AllocatorError::BroadcastFailed(Box::new(e)))?
                .respond_with_concurrent(None, move |request| {
                    let tx = tx.clone();
                    async move {
                        if let (peer_id, api::Request::WorkerOffer(offer)) = request {
                            let _ = tx.send((peer_id, offer)).await;
                        }

                        api::Response::WorkerOffer(worker_offer::Response {})
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

        // NOTE: Aggregate offers and select the best `num` using the GreedyOfferAggregator
        let mut offer_aggregator = pin!(GreedyOfferAggregator::new(
            ReceiverStream::new(rx),
            id,
            deadline,
            None, // TODO: configure max offers
            num,
            true, // strive for diversity by default
            bid,  // treat bid as max acceptable price for early return
        ));

        let offers = offer_aggregator.next().await;

        // NOTE: Always abort the response handler task to prevent resource leaks
        // The task should be cancelled regardless of whether we found an offer or not
        offer_handle.abort();

        match offers {
            Some(offers) if !offers.is_empty() => {
                // Create workers concurrently for selected offers
                let futures =
                    offers.into_iter().map(|(peer_id, offer)| {
                        let spec = spec.clone();
                        let network = self.network.clone();
                        async move {
                            Worker::create(offer.id, peer_id, spec, offer.price, network).await
                        }
                    });

                let workers = futures_util::future::join_all(futures).await;
                Ok(workers)
            }
            _ => Err(AllocatorError::NoOffersReceived),
        }
    }
}

#[derive(Debug, Error)]
enum CandidatesError {
    #[error("Already got a better offer from this peer")]
    Rejected,
    #[error("Already got enough and better offers")]
    Full,
}

struct Candidates {
    offers: Vec<(PeerId, WorkerOfferRequest)>,
    capacity: usize,
    diversity: bool,
}

impl Candidates {
    fn new(capacity: usize, diversity: bool) -> Self {
        Self {
            offers: Vec::with_capacity(capacity),
            capacity,
            diversity,
        }
    }

    fn try_insert(
        &mut self,
        peer_id: PeerId,
        offer: WorkerOfferRequest,
    ) -> Result<(), CandidatesError> {
        if self.diversity {
            // Find existing offer from this peer and reject if not better then old
            if let Some(i) = self.offers.iter().position(|(p, _)| *p == peer_id) {
                if offer.price < self.offers[i].1.price {
                    self.offers[i] = (peer_id, offer);
                    self.sort();

                    return Ok(());
                }

                return Err(CandidatesError::Rejected);
            }
        }

        // Add new offer if there's space
        if self.offers.len() < self.capacity {
            self.offers.push((peer_id, offer));
            self.sort();

            return Ok(());
        }

        // Replace worst offer if new one is better
        if let Some(worst_price) = self.offers.last().map(|(_, o)| o.price)
            && offer.price < worst_price
            && let Some(last) = self.offers.last_mut()
        {
            *last = (peer_id, offer);

            return Ok(());
        }

        Err(CandidatesError::Full)
    }

    fn sort(&mut self) {
        self.offers.sort_by(|a, b| a.1.price.total_cmp(&b.1.price));
    }

    fn len(&self) -> usize {
        self.offers.len()
    }

    fn worst_price(&self) -> Option<f64> {
        self.offers.last().map(|(_, o)| o.price)
    }

    fn offers(&self) -> &[(PeerId, WorkerOfferRequest)] {
        &self.offers
    }
}

impl From<Candidates> for Vec<(PeerId, WorkerOfferRequest)> {
    fn from(candidates: Candidates) -> Self {
        candidates.offers
    }
}

#[pin_project]
struct GreedyOfferAggregator<S> {
    #[pin]
    stream: S,
    #[pin]
    deadline: Sleep,
    candidates: Candidates,
    max_offers: Option<usize>,
    offers_received: usize,
    returned: bool,
    hard_deadline: tokio::time::Instant,
    upper_price: f64,
}

impl<S> GreedyOfferAggregator<S> {
    pub fn new(
        stream: S,
        _request_id: Uuid,
        deadline: Duration,
        max_offers: Option<usize>,
        desired: usize,
        diversity: bool,
        upper_price: f64,
    ) -> Self {
        let hard_deadline = tokio::time::Instant::now() + deadline;
        GreedyOfferAggregator {
            stream,
            deadline: tokio::time::sleep(deadline),
            candidates: Candidates::new(desired.max(1), diversity),
            max_offers,
            offers_received: 0,
            returned: false,
            hard_deadline,
            upper_price,
        }
    }
}

impl<S> Stream for GreedyOfferAggregator<S>
where
    S: Stream<Item = (PeerId, WorkerOfferRequest)> + Unpin,
{
    type Item = Vec<(PeerId, WorkerOfferRequest)>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        if *this.returned {
            return Poll::Ready(None);
        }

        // First check if we've reached max offers limit
        if let Some(max) = this.max_offers
            && *this.offers_received >= *max
        {
            tracing::debug!("Reached maximum offer limit");
            let candidates = std::mem::replace(this.candidates, Candidates::new(0, false));
            *this.returned = true;
            return Poll::Ready(Some(candidates.into()));
        }

        // Poll the deadline timer
        if this.deadline.as_mut().poll(cx).is_ready() {
            tracing::debug!("Deadline reached");
            let candidates = std::mem::replace(this.candidates, Candidates::new(0, false));
            *this.returned = true;
            return Poll::Ready(Some(candidates.into()));
        }

        // Poll for new offers
        loop {
            match this.stream.as_mut().poll_next(cx) {
                Poll::Ready(Some((peer_id, offer))) => {
                    *this.offers_received += 1;

                    let changed = this.candidates.try_insert(peer_id, offer.clone()).is_ok();

                    if changed {
                        // Update deadline to earliest expiry among candidates and hard deadline
                        let now = SystemTime::now();
                        let expiry_buffer = Duration::from_millis(100);
                        let mut new_deadline = *this.hard_deadline;
                        for (_, off) in this.candidates.offers().iter() {
                            if let Ok(time_until_expiry) = off.timeout.duration_since(now) {
                                let duration_until_expiry = if time_until_expiry > expiry_buffer {
                                    time_until_expiry - expiry_buffer
                                } else {
                                    Duration::from_millis(0)
                                };
                                let candidate = tokio::time::Instant::now() + duration_until_expiry;
                                if candidate < new_deadline {
                                    new_deadline = candidate;
                                }
                            }
                        }
                        this.deadline.as_mut().reset(new_deadline);

                        // Early return if we've reached desired count and max price <= upper bound
                        if this.candidates.len() >= this.candidates.capacity
                            && let Some(worst_price) = this.candidates.worst_price()
                            && worst_price <= *this.upper_price
                        {
                            let candidates =
                                std::mem::replace(this.candidates, Candidates::new(0, false));
                            *this.returned = true;
                            return Poll::Ready(Some(candidates.into()));
                        }
                    }

                    // Re-check termination conditions after processing this offer
                    continue;
                }
                Poll::Ready(None) => {
                    // Stream ended, return what we have
                    let candidates = std::mem::replace(this.candidates, Candidates::new(0, false));
                    *this.returned = true;
                    return Poll::Ready(Some(candidates.into()));
                }
                Poll::Pending => {
                    // No more offers available right now
                    return Poll::Pending;
                }
            }
        }
    }
}
