use std::{
    future::Future,
    pin::{Pin, pin},
    task::Poll,
    time::{Duration, SystemTime},
};

use futures_util::{Stream, StreamExt, future::join_all};
use hypha_messages::{
    WorkerSpec, api, request_worker,
    worker_offer::{self, Request as WorkerOfferRequest},
};
use hypha_network::{gossipsub::GossipsubInterface, request_response::RequestResponseInterfaceExt};
use hypha_resources::WeightedResourceEvaluator;
use libp2p::PeerId;
use pin_project::pin_project;
use thiserror::Error;
use tokio::{sync::mpsc, time::Sleep};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::{network::Network, scheduler_config::PriceRange, worker::Worker};

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
    /// Request `num` workers matching the given specification offered for prices within the
    /// requested range.
    fn request(
        &self,
        spec: WorkerSpec,
        price: PriceRange,
        deadline: Option<Duration>,
        num: usize,
    ) -> impl Future<Output = Result<Vec<Worker>, AllocatorError>> + Send;
}

/// Greedy allocator that selects the first worker offering the lowest price
pub struct GreedyWorkerAllocator {
    network: Network,
    evaluator: WeightedResourceEvaluator,
}

impl GreedyWorkerAllocator {
    // TODO: Consider request specific evaluator instead of one for all worker requests
    pub fn new(network: Network, evaluator: WeightedResourceEvaluator) -> Self {
        Self { network, evaluator }
    }
}

impl Allocator for GreedyWorkerAllocator {
    async fn request(
        &self,
        spec: WorkerSpec,
        price: PriceRange,
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

        // Set up channel for offers with their response channels
        let (tx, rx) = mpsc::channel(100);

        // NOTE: Set up a handler to receive offers and respond with Accepted/Rejected
        // The response is determined after aggregation, so we use oneshot channels to
        // communicate the decision back to each response handler
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
                            // Create a oneshot channel for receiving the decision
                            let (decision_tx, decision_rx) = tokio::sync::oneshot::channel();
                            let _ = tx.send((peer_id, offer, decision_tx)).await;

                            // Wait for the decision from the aggregator
                            match decision_rx.await {
                                Ok(accepted) => {
                                    if accepted {
                                        return api::Response::WorkerOffer(worker_offer::Response::Accepted);
                                    } else {
                                        return api::Response::WorkerOffer(worker_offer::Response::Rejected);
                                    }
                                }
                                Err(_) => {
                                    // Channel closed, default to rejected
                                    return api::Response::WorkerOffer(worker_offer::Response::Rejected);
                                }
                            }
                        }

                        api::Response::WorkerOffer(worker_offer::Response::Rejected)
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
                bid: price.bid,
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
            deadline,
            None, // TODO: configure max offers
            num,
            true, // TODO: Configure whether to strive for diversity
            price.max,
            self.evaluator,
        ));

        let result = offer_aggregator.next().await;

        // NOTE: Always abort the response handler task to prevent resource leaks
        // The task should be cancelled regardless of whether we found an offer or not
        offer_handle.abort();

        match result {
            Some((offers, rejected_decisions)) if !offers.is_empty() => {
                // NOTE: Send Rejected to all non-selected offers
                for decision_tx in rejected_decisions {
                    let _ = decision_tx.send(false);
                }

                // NOTE: While the worker instances created here are themselves futures,
                // it's the caller's responsibility to await them.
                #[allow(clippy::async_yields_async)]
                let workers = join_all(offers.into_iter().map(|(peer_id, offer, decision_tx)| {
                    let spec = spec.clone();
                    async move {
                        // Send Accepted response
                        let _ = decision_tx.send(true);

                        Worker::create(
                            offer.id,
                            peer_id,
                            spec,
                            offer.resources,
                            offer.price,
                            self.network.clone(),
                        )
                        .await
                    }
                }))
                .await;

                Ok(workers)
            }
            Some((_, rejected_decisions)) => {
                // No offers selected, reject all
                for decision_tx in rejected_decisions {
                    let _ = decision_tx.send(false);
                }
                Err(AllocatorError::NoOffersReceived)
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

struct Candidate {
    peer_id: PeerId,
    offer: WorkerOfferRequest,
    score: f64,
    decision_tx: tokio::sync::oneshot::Sender<bool>,
}

impl Candidate {
    fn new(
        peer_id: PeerId,
        offer: WorkerOfferRequest,
        score: f64,
        decision_tx: tokio::sync::oneshot::Sender<bool>,
    ) -> Self {
        Self {
            peer_id,
            offer,
            score,
            decision_tx,
        }
    }
}

struct Candidates {
    offers: Vec<Candidate>,
    capacity: usize,
    diversity: bool,
    rejected: Vec<tokio::sync::oneshot::Sender<bool>>,
}

impl Candidates {
    fn new(capacity: usize, diversity: bool) -> Self {
        Self {
            offers: Vec::with_capacity(capacity),
            capacity,
            diversity,
            rejected: Vec::new(),
        }
    }

    fn try_insert(&mut self, candidate: Candidate) -> Result<(), CandidatesError> {
        if self.diversity {
            // Find existing offer from this peer and reject if not better then old
            if let Some(i) = self
                .offers
                .iter()
                .position(|c| c.peer_id == candidate.peer_id)
            {
                if candidate.score < self.offers[i].score {
                    // Replace existing offer with better one, reject the old one
                    let old = std::mem::replace(&mut self.offers[i], candidate);
                    self.rejected.push(old.decision_tx);
                    self.sort();

                    return Ok(());
                }

                // Reject the new candidate
                self.rejected.push(candidate.decision_tx);
                return Err(CandidatesError::Rejected);
            }
        }

        // Add new offer if there's space
        if self.offers.len() < self.capacity {
            self.offers.push(candidate);
            self.sort();

            return Ok(());
        }

        // Replace worst offer if new one is better
        if let Some(last) = self.offers.last_mut()
            && candidate.score < last.score
        {
            let old = std::mem::replace(last, candidate);
            self.rejected.push(old.decision_tx);
            self.sort();

            return Ok(());
        }

        // Reject the candidate
        self.rejected.push(candidate.decision_tx);
        Err(CandidatesError::Full)
    }

    fn sort(&mut self) {
        self.offers.sort_by(|a, b| a.score.total_cmp(&b.score));
    }

    fn len(&self) -> usize {
        self.offers.len()
    }

    fn capacity(&self) -> usize {
        self.capacity
    }

    fn offers(&self) -> &[Candidate] {
        &self.offers
    }

    fn into_results(
        self,
    ) -> (
        Vec<(PeerId, WorkerOfferRequest, tokio::sync::oneshot::Sender<bool>)>,
        Vec<tokio::sync::oneshot::Sender<bool>>,
    ) {
        let accepted = self
            .offers
            .into_iter()
            .map(|candidate| (candidate.peer_id, candidate.offer, candidate.decision_tx))
            .collect();
        (accepted, self.rejected)
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
    evaluator: WeightedResourceEvaluator,
}

impl<S> GreedyOfferAggregator<S> {
    pub fn new(
        stream: S,
        deadline: Duration,
        max_offers: Option<usize>,
        desired: usize,
        diversity: bool,
        upper_price: f64,
        evaluator: WeightedResourceEvaluator,
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
            evaluator,
        }
    }
}

impl<S> Stream for GreedyOfferAggregator<S>
where
    S: Stream<Item = (PeerId, WorkerOfferRequest, tokio::sync::oneshot::Sender<bool>)> + Unpin,
{
    type Item = (
        Vec<(PeerId, WorkerOfferRequest, tokio::sync::oneshot::Sender<bool>)>,
        Vec<tokio::sync::oneshot::Sender<bool>>,
    );

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
            return Poll::Ready(Some(candidates.into_results()));
        }

        // Poll the deadline timer
        if this.deadline.as_mut().poll(cx).is_ready() {
            tracing::debug!("Deadline reached");
            let candidates = std::mem::replace(this.candidates, Candidates::new(0, false));
            *this.returned = true;
            return Poll::Ready(Some(candidates.into_results()));
        }

        // Poll for new offers
        loop {
            match this.stream.as_mut().poll_next(cx) {
                Poll::Ready(Some((peer_id, offer, decision_tx))) => {
                    *this.offers_received += 1;

                    if offer.price > *this.upper_price {
                        tracing::debug!(
                            peer_id = %peer_id,
                            offer_price = %offer.price,
                            max_price = %this.upper_price,
                            "Rejecting offer above max price"
                        );
                        // Immediately reject offers that are above max price
                        let _ = decision_tx.send(false);
                        continue;
                    }

                    let score = this.evaluator.evaluate(offer.price, &offer.resources);
                    let changed = this
                        .candidates
                        .try_insert(Candidate::new(peer_id, offer, score, decision_tx))
                        .is_ok();

                    if changed {
                        // Update deadline to earliest expiry among candidates and hard deadline
                        let now = SystemTime::now();
                        let expiry_buffer = Duration::from_millis(100);
                        let mut new_deadline = *this.hard_deadline;
                        for candidate in this.candidates.offers().iter() {
                            if let Ok(time_until_expiry) =
                                candidate.offer.timeout.duration_since(now)
                            {
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

                        // Early return if we've reached desired count
                        if this.candidates.len() >= this.candidates.capacity() {
                            let candidates =
                                std::mem::replace(this.candidates, Candidates::new(0, false));
                            *this.returned = true;
                            return Poll::Ready(Some(candidates.into_results()));
                        }
                    }

                    // Re-check termination conditions after processing this offer
                    continue;
                }
                Poll::Ready(None) => {
                    // Stream ended, return what we have
                    let candidates = std::mem::replace(this.candidates, Candidates::new(0, false));
                    *this.returned = true;
                    return Poll::Ready(Some(candidates.into_results()));
                }
                Poll::Pending => {
                    // No more offers available right now
                    return Poll::Pending;
                }
            }
        }
    }
}
