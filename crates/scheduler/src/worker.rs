use std::{
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, SystemTime},
};

use futures_util::FutureExt;
use hypha_messages::{JobSpec, WorkerSpec, api, renew_lease};
use hypha_network::request_response::{RequestResponseError, RequestResponseInterfaceExt};
use hypha_resources::Resources;
use libp2p::PeerId;
use thiserror::Error;
use tokio::{task::JoinHandle, time::sleep};
use tokio_retry::{
    Retry,
    strategy::{FixedInterval, jitter},
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
    #[error("Network error: {0}")]
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
    // NOTE: When reallocating a worker, we'll need to know the price to determine the cost of the new worker.
    #[allow(dead_code)]
    price: f64,
    // NOTE: We will need the resources to determine the capacity of the new worker and adjust the batch size accordingly.
    #[allow(dead_code)]
    resources: Resources,
    lease_handler: JoinHandle<Result<(), WorkerError>>,
}

impl Worker {
    pub async fn create(
        lease_id: Uuid,
        peer_id: PeerId,
        spec: WorkerSpec,
        resources: Resources,
        price: f64,
        network: Network,
    ) -> Self {
        let mut last_timeout: Option<SystemTime> = None;

        let lease_handler: JoinHandle<Result<(), WorkerError>> = tokio::spawn({
            let network = network.clone();
            async move {
                loop {
                    tracing::debug!(%lease_id, %peer_id, "Refreshing lease");

                    let remaining_time = if let Some(last_timeout) = last_timeout {
                        last_timeout
                            .duration_since(SystemTime::now())
                            .unwrap_or(Duration::from_secs(1))
                    } else {
                        Duration::from_secs(1)
                    };
                    // NOTE: We retry network errors until the remaining time has elapsed.
                    let retry_strategy = FixedInterval::from_millis(200)
                        .map(jitter)
                        .take(remaining_time.as_millis() as usize / 200);

                    let result = Retry::spawn(retry_strategy, || {
                        let network = network.clone();
                        async move {
                            network
                                .request::<api::Codec>(
                                    peer_id,
                                    api::Request::RenewLease(renew_lease::Request { id: lease_id }),
                                )
                                .await
                        }
                    })
                    .await;

                    match result {
                        Ok(api::Response::RenewLease(renew_lease::Response::Renewed {
                            timeout,
                            ..
                        })) => {
                            last_timeout = Some(timeout);
                            let duration = timeout
                                .duration_since(SystemTime::now())
                                .unwrap_or(Duration::from_secs(6));

                            // Note: We don't wait for the full lease duration, but rather a
                            // fraction of it to ensure timely renewal before the lease expires.
                            let safe_duration = duration / 3 * 2;

                            tracing::debug!(
                                duration = duration.as_millis(),
                                safe_duration = safe_duration.as_millis(),
                                %lease_id,
                                %peer_id,
                                "Lease renewed, renewing in {}ms",
                                safe_duration.as_millis()
                            );

                            sleep(safe_duration).await;
                        }
                        Ok(api::Response::RenewLease(renew_lease::Response::NotFound)) => {
                            tracing::error!(
                                %lease_id,
                                %peer_id,
                                "Lease renewal failed: lease not found"
                            );

                            return Err(WorkerError::LeaseExpired);
                        }
                        Ok(api::Response::RenewLease(renew_lease::Response::Failed)) => {
                            tracing::error!(%lease_id, %peer_id, "Lease renewal failed");

                            return Err(WorkerError::LeaseExpired);
                        }
                        Ok(api::Response::RenewLease(renew_lease::Response::Forbidden)) => {
                            tracing::error!(%lease_id, %peer_id, "Lease renewal forbidden");

                            return Err(WorkerError::LeaseExpired);
                        }
                        Ok(response) => {
                            tracing::error!(
                                %lease_id,
                                %peer_id,
                                response = ?response,
                                "Lease renewal returned unexpected response"
                            );

                            return Err(WorkerError::LeaseExpired);
                        }
                        Err(error) => {
                            tracing::warn!(
                                %lease_id,
                                %peer_id,
                                error = %error,
                                "Lease renewal failed after retries"
                            );

                            return Err(WorkerError::NetworkError(error));
                        }
                    }
                }
            }
        });

        Self {
            lease_id,
            peer_id,
            spec,
            resources,
            price,
            lease_handler,
        }
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn lease_id(&self) -> Uuid {
        self.lease_id
    }

    pub fn price(&self) -> f64 {
        self.price
    }

    pub fn spec(&self) -> &WorkerSpec {
        &self.spec
    }

    pub fn resources(&self) -> &Resources {
        &self.resources
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

#[cfg(test)]
pub struct TestWorkerBuilder {
    lease_id: Uuid,
    peer_id: PeerId,
    spec: WorkerSpec,
    resources: Resources,
    price: f64,
    lease_handler: Option<JoinHandle<Result<(), WorkerError>>>,
}

#[cfg(test)]
impl TestWorkerBuilder {
    pub fn new() -> Self {
        Self {
            lease_id: Uuid::new_v4(),
            peer_id: PeerId::random(),
            spec: WorkerSpec {
                resources: Resources::default(),
                executor: Vec::new(),
            },
            resources: Resources::default(),
            price: 1.0,
            lease_handler: None,
        }
    }

    pub fn with_lease_handler(
        mut self,
        lease_handler: JoinHandle<Result<(), WorkerError>>,
    ) -> Self {
        self.lease_handler = Some(lease_handler);
        self
    }

    pub fn with_peer_id(mut self, peer_id: PeerId) -> Self {
        self.peer_id = peer_id;
        self
    }

    pub fn with_spec(mut self, spec: WorkerSpec) -> Self {
        self.spec = spec;
        self
    }

    pub fn with_resources(mut self, resources: Resources) -> Self {
        self.resources = resources;
        self
    }

    pub fn with_price(mut self, price: f64) -> Self {
        self.price = price;
        self
    }

    pub fn with_lease_id(mut self, lease_id: Uuid) -> Self {
        self.lease_id = lease_id;
        self
    }

    pub fn build(self) -> Worker {
        Worker {
            lease_id: self.lease_id,
            peer_id: self.peer_id,
            spec: self.spec,
            resources: self.resources,
            price: self.price,
            lease_handler: self.lease_handler.unwrap_or_else(|| {
                tokio::spawn(async move {
                    futures_util::future::pending::<()>().await;
                    Ok(())
                })
            }),
        }
    }
}
