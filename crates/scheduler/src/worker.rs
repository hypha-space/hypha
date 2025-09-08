use std::time::{Duration, SystemTime};

use hypha_messages::{
    JobSpec, Request, Response, WorkerSpec, dispatch_job, job_status, renew_lease,
};
use hypha_network::request_response::{
    RequestResponseError, RequestResponseInterface, RequestResponseInterfaceExt,
};
use libp2p::PeerId;
use thiserror::Error;
use tokio::{
    sync::mpsc::{self, Receiver},
    task::JoinSet,
    time::sleep,
};
use uuid::Uuid;

use crate::network::Network;

#[derive(Debug, Clone)]
pub struct WorkerInfo {
    pub peer_id: PeerId,
    pub capabilities: WorkerSpec,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: Uuid,
    pub spec: JobSpec,
}

#[derive(Debug, Clone)]
pub enum FailureReason {
    LeaseExpired,
    JobFailed(String),
    WorkerDisconnected,
}

#[derive(Debug, Clone)]
pub struct WorkerFailure {
    pub peer_id: PeerId,
    pub lease_id: Uuid,
    pub reason: FailureReason,
}

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("Worker disconnected")]
    Disconnected,
    #[error("Job dispatch failed: {0}")]
    DispatchFailed(String),
    #[error("Lease expired")]
    LeaseExpired,
    #[error("Network error")]
    NetworkError(#[from] RequestResponseError),
}

// TODO: Define JobHandle struct if needed for job management

/// A Worker handle that manages lease renewal and job dispatch
pub struct Worker {
    lease_id: Uuid,
    peer_id: PeerId,
    // NOTE: We'll need the spec and price to re-allocate a worker in case of failure.
    #[allow(dead_code)]
    spec: WorkerSpec,
    #[allow(dead_code)]
    price: f64,
    network: Network,
    jobs: JoinSet<()>,
}

impl Worker {
    pub(crate) async fn create(
        lease_id: Uuid,
        peer_id: PeerId,
        spec: WorkerSpec,
        price: f64,
        network: Network,
    ) -> Self {
        let mut jobs = JoinSet::new();

        jobs.spawn({
            let peer_id = peer_id;
            let network = network.clone();
            async move {
                // NOTE: Refresh at 75% of lease duration to ensure we renew before expiry
                loop {
                    tracing::info!(lease_id =%lease_id, peer_id = %peer_id, "Refreshing lease");
                    match network
                        .request(
                            peer_id,
                            Request::RenewLease(renew_lease::Request { id: lease_id }),
                        )
                        .await
                    {
                        Ok(Response::RenewLease(renew_lease::Response::Renewed {
                            id: _,
                            timeout,
                        })) => {
                            // Handle successful response

                            // TODO: Make the min refresg configurable

                            let duration = timeout
                                .duration_since(SystemTime::now())
                                .unwrap_or(Duration::from_millis(10000))
                                .min(Duration::from_millis(10000));

                            let safe_duration = duration * 3 / 4;

                            tracing::info!(
                                duration = duration.as_millis(),
                                safe_duration = safe_duration.as_millis(),
                                lease_id = %lease_id,
                                "Lease renewed, renewing in {}ms",
                                safe_duration.as_millis()
                            );
                            sleep(duration).await;
                        }
                        Ok(Response::RenewLease(renew_lease::Response::Failed)) => {
                            // Handle failed response
                            // TODO: Handle Error
                            tracing::error!("Failed to renew lease");
                            break;
                        }
                        Err(error) => {
                            // Handle error
                            // TODO: Handle Error
                            tracing::error!("Error renewing lease: {:?}", error);
                            break;
                        }
                        _ => {
                            // Handle unexpected response
                            // TODO: Handle Error
                            tracing::error!("Unexpected response");
                            break;
                        }
                    }
                }
            }
        });

        Self {
            lease_id,
            peer_id,
            spec,
            price,
            network,
            jobs,
        }
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn lease_id(&self) -> Uuid {
        self.lease_id
    }

    pub async fn dispatch(
        &mut self,
        job_spec: JobSpec,
    ) -> Result<Receiver<hypha_messages::JobStatus>, WorkerError> {
        // NOTE: We dispatch the job to the worker using the DispatchJob request
        // The worker will acknowledge with success/failure and handle the job execution
        let id = Uuid::new_v4();
        let (tx, rx) = mpsc::channel(100);

        // NOTE: Create job status handler for the job
        let id_clone = id.clone();
        self.jobs.spawn(
            self.network
                .on(move |req: &Request| {
                    matches!(
                        req,
                        Request::JobStatus(
                        job_status::Request { job_id, .. }
                    ) if job_id == &id_clone
                    )
                })
                .into_stream()
                .await
                .map_err(WorkerError::from)?
                .respond_with_concurrent(None, move |request| {
                    let tx = tx.clone();
                    async move {
                        if let (_peer_id, Request::JobStatus(job_status::Request { status, .. })) =
                            request
                        {
                            let _ = tx.send(status).await;
                        }

                        Response::JobStatus(job_status::Response {})
                    }
                }),
        );

        // NOTE: Dispatch job status to the worker
        match self
            .network
            .request(
                self.peer_id,
                Request::DispatchJob(dispatch_job::Request { id, spec: job_spec }),
            )
            .await
        {
            Ok(Response::DispatchJob(dispatch_job::Response::Dispatched { id, timeout })) => {
                tracing::info!(
                    job_id = %id,
                    peer_id = %self.peer_id,
                    timeout = ?timeout,
                    "Job successfully dispatched to worker"
                );
                // TODO: Consider sending this also as a status event along with the updates from the worker
            }
            Ok(Response::DispatchJob(dispatch_job::Response::Failed)) => {
                tracing::error!(
                    job_id = %id,
                    peer_id = %self.peer_id,
                    "Worker failed to accept job"
                );

                return Err(WorkerError::DispatchFailed(
                    "Worker rejected job".to_string(),
                ));
            }
            Ok(_) => {
                tracing::error!(
                    job_id = %id,
                    peer_id = %self.peer_id,
                    "Unexpected response type from worker"
                );

                return Err(WorkerError::DispatchFailed(
                    "Unexpected response type".to_string(),
                ));
            }
            Err(e) => {
                tracing::error!(
                    job_id = %id,
                    peer_id = %self.peer_id,
                    error = ?e,
                    "Network error while dispatching job"
                );

                return Err(WorkerError::DispatchFailed(format!("Network error: {}", e)));
            }
        };

        Ok(rx)
    }
}
