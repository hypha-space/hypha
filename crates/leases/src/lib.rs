use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Error)]
#[error("Ledger error: {0}")]
pub enum LedgerError {
    #[error("Lease not found: {0}")]
    LeaseNotFound(Uuid),
}

/// A lease entry in the ledger
#[derive(Debug, Clone)]
pub struct Lease<T> {
    pub id: Uuid,
    pub leasable: T,
    timeout: Instant,
}

impl<T> Lease<T> {
    /// Get the timeout as SystemTime for network serialization
    /// Note: This converts from Instant to SystemTime and may not be perfectly accurate
    /// if significant time has passed since the lease was created
    pub fn timeout_as_system_time(&self) -> SystemTime {
        let now_instant = Instant::now();
        let now_system = SystemTime::now();

        if self.timeout > now_instant {
            // Lease hasn't expired yet
            let remaining = self.timeout - now_instant;
            now_system + remaining
        } else {
            // Lease has already expired
            now_system
        }
    }

    /// Check if this lease has expired
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.timeout
    }
}

/// Generic ledger that can hold any type of lease
#[derive(Clone)]
pub struct Ledger<T>
where
    T: Clone + Send + Sync,
{
    leases: Arc<RwLock<HashMap<Uuid, Lease<T>>>>,
}

impl<T> Default for Ledger<T>
where
    T: Clone + Send + Sync,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Ledger<T>
where
    T: Clone + Send + Sync,
{
    /// Create a new ledger
    pub fn new() -> Self {
        Self {
            leases: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn insert(&self, leasable: T, duration: Duration) -> Result<Lease<T>, LedgerError> {
        tracing::debug!("insert");
        let mut leases = self.leases.write().await;

        let id = Uuid::new_v4();
        let timeout = Instant::now() + duration;

        let lease = Lease {
            id,
            leasable,
            timeout,
        };

        leases.insert(id, lease.clone());

        Ok(lease)
    }

    pub async fn get(&self, id: &Uuid) -> Result<Lease<T>, LedgerError> {
        self.leases
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or(LedgerError::LeaseNotFound(*id))
    }

    pub async fn remove(&self, id: &Uuid) -> Result<Lease<T>, LedgerError> {
        let mut leases = self.leases.write().await;

        let lease = leases.remove(id).ok_or(LedgerError::LeaseNotFound(*id))?;

        tracing::debug!("Removed lease {id}");
        Ok(lease)
    }

    pub async fn renew(&self, id: &Uuid, duration: Duration) -> Result<Lease<T>, LedgerError> {
        let mut leases = self.leases.write().await;

        let lease = leases.get_mut(id).ok_or(LedgerError::LeaseNotFound(*id))?;

        // NOTE: Renewing a lease updates the timeout to now + duration instead of extending the
        // original timeout to make it easier to reason about the lease's expiration time across
        // system boundaries.
        lease.timeout = Instant::now() + duration;

        Ok(lease.clone())
    }

    pub async fn list(&self) -> Vec<Lease<T>> {
        self.leases.read().await.values().cloned().collect()
    }

    /// Get all expired leases
    pub async fn list_expired(&self) -> Vec<Lease<T>> {
        let leases = self.leases.read().await;
        let now = Instant::now();

        leases
            .values()
            .filter(|lease| lease.timeout <= now)
            .cloned()
            .collect()
    }
}
