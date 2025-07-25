use std::{future::Future, time::Duration};

use hypha_leases::{Lease, Ledger, LedgerError};
use hypha_messages::Requirement;
use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::resources::{extract_compute_resource_requirements, extract_other_resource_requirements, ResourceManager, ResourceManagerError};

#[derive(Debug, Clone, Error)]
#[error("lease error")]
pub enum LeaseError {
    #[error("Resources reservation failed")]
    ResourcesReservation(#[from] ResourceManagerError),
    #[error("Ledger error: {0}")]
    Ledger(#[from] LedgerError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLease {
    pub peer_id: PeerId,
    pub reservation: Uuid,
}

/// Manages resource leases with atomic resource reservation
pub trait LeaseManager: Send + Sync + Clone {
    type Leasable: Send + Sync + Clone;

    /// Request a new lease with requirements and duration
    fn request(
        &mut self,
        peer_id: PeerId,
        requirements: Vec<Requirement>,
        duration: Duration,
    ) -> impl Future<Output = Result<Lease<Self::Leasable>, LeaseError>> + Send;

    fn get(
        &self,
        id: &Uuid,
    ) -> impl Future<Output = Result<Lease<Self::Leasable>, LeaseError>> + Send;

    /// Find lease by peer ID
    fn get_by_peer(
        &self,
        peer_id: &PeerId,
    ) -> impl Future<Output = Result<Lease<Self::Leasable>, LeaseError>> + Send;

    /// Remove a lease and release its associated resources
    fn remove(
        &mut self,
        id: &Uuid,
    ) -> impl Future<Output = Result<Lease<Self::Leasable>, LeaseError>> + Send;

    /// Renew a lease
    fn renew(
        &self,
        id: &Uuid,
        duration: Duration,
    ) -> impl Future<Output = Result<Lease<Self::Leasable>, LeaseError>> + Send;

    /// Proceed leases and return expired leases
    fn proceed(
        &mut self,
    ) -> impl Future<Output = Result<Vec<Lease<Self::Leasable>>, LeaseError>> + Send;
}

/// Thread-safe lease manager implementation
#[derive(Clone)]
pub struct ResourceLeaseManager<R>
where
    R: ResourceManager + Send + Sync + Clone,
{
    resource_manager: R,
    ledger: Ledger<ResourceLease>,
}

impl<R> ResourceLeaseManager<R>
where
    R: ResourceManager + Send + Sync + Clone,
{
    pub fn new(resource_manager: R) -> Self {
        Self {
            resource_manager,
            ledger: Ledger::new(),
        }
    }
}

impl<R> LeaseManager for ResourceLeaseManager<R>
where
    R: ResourceManager + Send + Sync + Clone,
{
    type Leasable = ResourceLease;
    async fn request(
        &mut self,
        peer_id: PeerId,
        requirements: Vec<Requirement>,
        duration: Duration,
    ) -> Result<Lease<ResourceLease>, LeaseError> {
        let compute_resources = extract_compute_resource_requirements(&requirements);
        let other_resources = extract_other_resource_requirements(&requirements);
        let reservation = self.resource_manager.reserve(compute_resources, other_resources).await?;

        let result = self
            .ledger
            .insert(
                ResourceLease {
                    peer_id,
                    reservation,
                },
                duration,
            )
            .await
            .map_err(LeaseError::from);

        if result.is_err() {
            self.resource_manager.release(reservation).await?;
        }

        result
    }

    async fn get(&self, id: &Uuid) -> Result<Lease<ResourceLease>, LeaseError> {
        self.ledger.get(id).await.map_err(LeaseError::from)
    }

    async fn get_by_peer(&self, peer_id: &PeerId) -> Result<Lease<ResourceLease>, LeaseError> {
        // NOTE: For now, we find the first active lease for this peer
        // In a production system, this might need to be more sophisticated
        let all_leases = self.ledger.list().await;

        for lease in all_leases {
            if lease.leasable.peer_id == *peer_id {
                return Ok(lease);
            }
        }

        Err(LeaseError::Ledger(LedgerError::LeaseNotFound(
            uuid::Uuid::new_v4(), // Use a dummy UUID for the error
        )))
    }

    async fn remove(&mut self, id: &Uuid) -> Result<Lease<ResourceLease>, LeaseError> {
        tracing::debug!("Removing lease {id}");

        let lease = self.ledger.remove(id).await?;
        self.resource_manager
            .release(lease.leasable.reservation)
            .await?;

        tracing::debug!("Removed lease {id} and released resources");

        Ok(lease)
    }

    async fn renew(
        &self,
        id: &Uuid,
        duration: Duration,
    ) -> Result<Lease<ResourceLease>, LeaseError> {
        self.ledger
            .renew(id, duration)
            .await
            .map_err(LeaseError::from)
    }

    async fn proceed(&mut self) -> Result<Vec<Lease<ResourceLease>>, LeaseError> {
        let expired_leases = self.ledger.list_expired().await;

        let mut removed_leases = Vec::with_capacity(expired_leases.len());
        for lease in expired_leases {
            // TODO: We will probably need to handle more cases here to err in the right cases as
            // there are some conditions we can probably ignore or handle gracefully.
            match self.remove(&lease.id).await {
                Ok(lease) => removed_leases.push(lease),
                Err(LeaseError::Ledger(LedgerError::LeaseNotFound(_))) => {
                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Ok(removed_leases)
    }
}
