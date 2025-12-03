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
        let lease_handler: JoinHandle<Result<(), WorkerError>> = tokio::spawn({
            let network = network.clone();
            async move {
                loop {
                    tracing::debug!(%lease_id, %peer_id, "Refreshing lease");
                    match network
                        .request::<api::Codec>(
                            peer_id,
                            api::Request::RenewLease(renew_lease::Request { id: lease_id }),
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

                            tracing::debug!(
                                duration = duration.as_millis(),
                                safe_duration = safe_duration.as_millis(),
                                %lease_id,
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
