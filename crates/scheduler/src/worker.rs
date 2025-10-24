use std::{
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, SystemTime},
};

use futures_util::FutureExt;
use hypha_messages::{JobSpec, WorkerSpec, api, renew_lease};
use hypha_network::request_response::{RequestResponseError, RequestResponseInterfaceExt};
use libp2p::PeerId;
use thiserror::Error;
use tokio::{task::JoinHandle, time::sleep};
use uuid::Uuid;

use crate::{allocator::AllocatedWorker, network::Network};

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

    lease_handler: JoinHandle<Result<(), WorkerError>>,
}

impl Worker {
    pub async fn create(allocated_worker: AllocatedWorker, network: Network) -> Self {
        let lease_handler: JoinHandle<Result<(), WorkerError>> = tokio::spawn({
            let network = network.clone();
            async move {
                loop {
                    tracing::info!(lease_id = %allocated_worker.lease_id, peer_id = %allocated_worker.peer_id, "Refreshing lease");
                    match network
                        .request::<api::Codec>(
                            allocated_worker.peer_id,
                            api::Request::RenewLease(renew_lease::Request {
                                id: allocated_worker.lease_id,
                            }),
                        )
                        .await
                    {
                        Ok(api::Response::RenewLease(renew_lease::Response::Renewed {
                            id: _,
                            timeout,
                        })) => {
                            // Handle successful response

                            // TODO: Make the min refresh configurable

                            let duration = timeout
                                .duration_since(SystemTime::now())
                                .unwrap_or(Duration::from_secs(6));

                            let safe_duration = duration / 3 * 2;

                            tracing::info!(
                                duration = duration.as_millis(),
                                safe_duration = safe_duration.as_millis(),
                                lease_id = %allocated_worker.lease_id,
                                "Lease renewed, renewing in {}ms",
                                safe_duration.as_millis()
                            );

                            sleep(safe_duration).await;
                        }
                        Ok(api::Response::RenewLease(renew_lease::Response::Failed)) => {
                            // Handle failed response
                            return Err(WorkerError::LeaseExpired);
                        }
                        Err(error) => {
                            // Handle error
                            return Err(WorkerError::NetworkError(error));
                        }
                        _ => {
                            // Handle unexpected response
                            return Err(WorkerError::DispatchFailed(
                                "Unexpected response".to_string(),
                            ));
                        }
                    }
                }
            }
        });

        Self {
            lease_id: allocated_worker.lease_id,
            peer_id: allocated_worker.peer_id,
            spec: allocated_worker.spec,
            price: allocated_worker.price,
            lease_handler,
        }
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn lease_id(&self) -> Uuid {
        self.lease_id
    }
}

impl Future for Worker {
    type Output = Result<(), WorkerError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.lease_handler
            .poll_unpin(cx)
            .map_err(|_| WorkerError::Disconnected)?
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.lease_handler.abort();
    }
}
