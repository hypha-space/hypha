use futures_util::StreamExt;
use hypha_messages::{self, Request, Response, dispatch_job, renew_lease};
use hypha_network::{
    gossipsub::{self, GossipsubInterface},
    request_response::{
        RequestResponseError, RequestResponseInterface, RequestResponseInterfaceExt,
    },
    utils::Batched,
};
use libp2p::PeerId;
use ordered_float::OrderedFloat;
use thiserror::Error;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::{
    job_manager::{JobManager, JobManagerError},
    lease_manager::{LeaseError, LeaseManager},
    network::Network,
    request_evaluator::RequestEvaluator,
};

const WORKER_TOPIC: &str = "hypha/worker";
// NOTE: Windowing configuration for batching incoming gossipsub messages
// This allows proper handling of multiple schedulers by batching advertisements
const WINDOW_LIMIT: usize = 100;
const WINDOW_WAIT: std::time::Duration = std::time::Duration::from_millis(200);
const OFFER_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
const LEASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, Error)]
#[error("lease error")]
pub enum ArbiterError {
    #[error("Gossipsub error: {0}")]
    Gossipsub(#[from] gossipsub::GossipsubError),
    #[error("Lease error: {0}")]
    LeaseManager(#[from] LeaseError),
    #[error("Network error: {0}")]
    Network(#[from] RequestResponseError),
    #[error("Task manager error: {0}")]
    JobManager(#[from] JobManagerError),
}

/// Orchestrates resource allocation by coordinating between network communication,
/// offer strategy, and lease management
pub struct Arbiter<L, R>
where
    L: LeaseManager<Leasable = crate::lease_manager::ResourceLease> + 'static,
    R: RequestEvaluator,
{
    network: Network,
    lease_manager: L,
    request_evaluator: R,
    job_manager: JobManager,
    tasks: JoinSet<()>,
}

impl<L, R> Arbiter<L, R>
where
    L: LeaseManager<Leasable = crate::lease_manager::ResourceLease> + 'static,
    R: RequestEvaluator,
{
    /// Create a new Arbiter with default strategy and system clock
    pub fn new(
        lease_manager: L,
        request_evaluator: R,
        network: Network,
        job_manager: JobManager,
    ) -> Self {
        Self {
            network,
            lease_manager,
            request_evaluator,
            job_manager,
            tasks: JoinSet::new(),
        }
    }

    pub async fn run(mut self, cancel: CancellationToken) -> Result<(), ArbiterError> {
        let requests = self.network.subscribe(WORKER_TOPIC).await?;

        // NOTE: Use windowed stream to batch incoming gossipsub messages
        // This enables proper selection when multiple schedulers broadcast
        let mut requests_batched = Box::pin(Batched::new(requests, WINDOW_LIMIT, WINDOW_WAIT));

        // Spawn lease pruning job
        let mut lease_manager = self.lease_manager.clone();
        self.tasks.spawn(async move {
            let mut interval = tokio::time::interval(PRUNE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if let Ok(expired) = lease_manager.proceed().await
                    && !expired.is_empty()
                {
                    tracing::debug!("Pruned {} expired leases", expired.len());
                }
            }
        });

        let renewal_requests = self
            .network
            .on(|req: &Request| matches!(req, Request::RenewLease(_)))
            .into_stream()
            .await?;

        let lease_manager = self.lease_manager.clone();
        self.tasks.spawn(async move {
            renewal_requests
                .respond_with_concurrent(None, move |(peer_id, request)| {
                    let lease_manager = lease_manager.clone();
                    async move {
                        if let Request::RenewLease(renew_lease::Request { id }) = request {
                            // NOTE: Validate that only the owning peer can renew the lease
                            match lease_manager.get(&id).await {
                                Ok(lease) => {
                                    if lease.leasable.peer_id == peer_id {
                                        match lease_manager.renew(&id, LEASE_TIMEOUT).await {
                                            Ok(lease) => {
                                                return Response::RenewLease(
                                                    renew_lease::Response::Renewed {
                                                        id,
                                                        timeout: lease.timeout_as_system_time(),
                                                    },
                                                );
                                            }
                                            Err(e) => {
                                                tracing::error!(
                                                    lease_id = %id,
                                                    scheduler_id = %peer_id,
                                                    error = ?e,
                                                    "Failed to renew lease"
                                                );
                                            }
                                        }
                                    } else {
                                        tracing::warn!(
                                            lease_id = %id,
                                            scheduler_id = %peer_id,
                                            owner_id = %lease.leasable.peer_id,
                                            "Rejecting renewal: peer does not own lease"
                                        );
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        lease_id = %id,
                                        scheduler_id = %peer_id,
                                        error = ?e,
                                        "Rejecting renewal: lease not found"
                                    );
                                }
                            }
                        }
                        Response::RenewLease(renew_lease::Response::Failed)
                    }
                })
                .await
        });

        // NOTE: Handle DispatchJob requests from schedulers
        let dispatch_requests = self
            .network
            .on(|req: &Request| matches!(req, Request::DispatchJob(_)))
            .into_stream()
            .await?;

        let job_manager = self.job_manager.clone();
        let lease_manager = self.lease_manager.clone();
        self.tasks.spawn(async move {
            dispatch_requests
                .respond_with_concurrent(None, move |(peer_id, request)| {
                    let mut job_manager = job_manager.clone();
                    let lease_manager = lease_manager.clone();
                    async move {
                        if let Request::DispatchJob(dispatch_job::Request { id, spec }) = request {
                            // NOTE: Validate that the scheduler has a valid lease
                            match lease_manager.get_by_peer(&peer_id).await {
                                Ok(lease) => {
                                    tracing::info!(
                                        job_id = %id,
                                        scheduler_id = %peer_id,
                                        lease_id = %lease.id,
                                        "Scheduler has valid lease, dispatching job"
                                    );

                                    match job_manager.execute(id, spec, lease.id, peer_id).await {
                                        Ok(()) => {
                                            tracing::info!(
                                                job_id = %id,
                                                scheduler_id = %peer_id,
                                                "Task successfully dispatched"
                                            );
                                            Response::DispatchJob(
                                                dispatch_job::Response::Dispatched {
                                                    id,
                                                    // NOTE: We currently acknowledge with the existing lease timeout.
                                                    // TODO: Consider returning the renewed/expected timeout from JobManager::execute.
                                                    timeout: lease.timeout_as_system_time(),
                                                },
                                            )
                                        }
                                        Err(e) => {
                                            tracing::error!(
                                                job_id = %id,
                                                scheduler_id = %peer_id,
                                                error = ?e,
                                                "Failed to dispatch job"
                                            );
                                            Response::DispatchJob(dispatch_job::Response::Failed)
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        job_id = %id,
                                        scheduler_id = %peer_id,
                                        error = ?e,
                                        "Rejecting job from scheduler without valid lease"
                                    );
                                    Response::DispatchJob(dispatch_job::Response::Failed)
                                }
                            }
                        } else {
                            Response::DispatchJob(dispatch_job::Response::Failed)
                        }
                    }
                })
                .await
        });

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::debug!("Arbiter cancelled");
                    break;
                },
                Some(request_batch) = requests_batched.next() => {
                    tracing::debug!("Got a batch of requests");
                    let parsed_requests: Vec<(PeerId, hypha_messages::request_worker::Request)> = request_batch
                        .into_iter()
                        .filter_map(|message| {
                            if let Ok(m) = message
                                && let Ok(request) = ciborium::from_reader::<hypha_messages::request_worker::Request, _>(&m.data[..]) {
                                    return Some((m.source, request));
                            }
                            None
                        })
                        .collect();

                    if parsed_requests.is_empty() {
                        tracing::debug!("No valid requests in batch");
                        continue;
                    }

                    tracing::debug!("Received {} requests", parsed_requests.len());
                    self.process_requests(parsed_requests).await;
                },
                else => {
                    tracing::debug!("Request stream ended");
                    break;
                }
            }
        }

        // Shutdown gracefully
        self.job_manager.shutdown().await;

        self.tasks.abort_all();

        while let Some(result) = self.tasks.join_next().await {
            if let Err(e) = result
                && !e.is_cancelled()
            {
                tracing::error!("Task failed: {:?}", e);
            }
        }

        Ok(())
    }

    async fn process_requests(
        &mut self,
        requests: Vec<(PeerId, hypha_messages::request_worker::Request)>,
    ) {
        // NOTE: Score and sort requests, then greedily request a lease and offer it to the peer.
        // Offers will have a short lifetime and the offer must be accepted within the time frame
        // to extend the lease.
        let mut scored_requests: Vec<_> = requests
            .iter()
            .map(|(peer_id, request)| {
                let score = self.request_evaluator.score(request);

                (score, *peer_id, request.clone())
            })
            .collect();

        scored_requests.sort_by_key(|(score, _, _)| OrderedFloat(-score));

        for (_, peer_id, request) in scored_requests {
            tracing::info!("Processing request from peer {}", peer_id);
            if let Ok(lease) = self
                .lease_manager
                .request(peer_id, request.spec.requirements.clone(), OFFER_TIMEOUT)
                .await
            {
                let offer = hypha_messages::worker_offer::Request {
                    id: lease.id,
                    request_id: request.id,
                    // TODO: Implement price calculation based on resource requirements and
                    // current utilization.
                    price: request.bid,
                    timeout: lease.timeout_as_system_time(),
                };

                let mut lease_manager = self.lease_manager.clone();
                let network = self.network.clone();
                self.tasks.spawn(async move {
                    // NOTE: Send offer to peer
                    if let Err(err) = network
                        .request(peer_id, hypha_messages::Request::WorkerOffer(offer.clone()))
                        .await
                    {
                        tracing::error!(
                            request_id = ?request.id,
                            peer_id = ?peer_id,
                            error = %err,
                            "Failed to send WorkerOffer"
                        );

                        if let Err(e) = lease_manager.remove(&lease.id).await {
                            tracing::error!(
                                lease_id = ?lease.id,
                                error = %e,
                                "Failed to remove lease after offer send failure"
                            );
                        }
                    }
                });
            }
        }
    }
}
