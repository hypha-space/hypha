use std::{collections::HashMap, future::Future, sync::Arc};

use hypha_resources::Resources;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Error)]
#[error("Resource manager error")]
pub enum ResourceManagerError {
    #[error("Insufficient resources")]
    InsufficientResources,
    #[error("Unknown resource reservation")]
    UnknownReservation,
}

pub trait ResourceManager: Send + Sync {
    /// Returns a reference to the current available resources.
    fn compute_resources(&self) -> impl Future<Output = Resources> + Send;
    fn reserve(
        &self,
        compute_resources: Resources,
    ) -> impl Future<Output = Result<Uuid, ResourceManagerError>> + Send;
    fn release(
        &self,
        id: Uuid,
    ) -> impl Future<Output = Result<Resources, ResourceManagerError>> + Send;
}

#[derive(Debug)]
struct StaticResourceManagerState {
    compute_resources: Resources,
    reservations: HashMap<Uuid, Resources>,
}

#[derive(Debug, Clone)]
pub struct StaticResourceManager {
    state: Arc<RwLock<StaticResourceManagerState>>,
}

impl StaticResourceManager {
    pub fn new(compute_resources: Resources) -> Self {
        StaticResourceManager {
            state: Arc::new(RwLock::new(StaticResourceManagerState {
                compute_resources,
                reservations: HashMap::default(),
            })),
        }
    }
}

impl ResourceManager for StaticResourceManager {
    async fn compute_resources(&self) -> Resources {
        let state = self.state.read().await;
        state.compute_resources - state.reservations.values().sum()
    }

    async fn reserve(&self, compute_resources: Resources) -> Result<Uuid, ResourceManagerError> {
        let available = self.compute_resources().await;
        tracing::info!(
            "Reserving resources: {:?} of {:?}",
            compute_resources,
            available,
        );

        if available >= compute_resources {
            let mut state = self.state.write().await;
            // Re-check after acquiring write lock
            let current_available = state.compute_resources - state.reservations.values().sum();
            if current_available >= compute_resources {
                let id = Uuid::new_v4();
                state.reservations.insert(id, compute_resources);
                return Ok(id);
            }
        }

        Err(ResourceManagerError::InsufficientResources)
    }

    async fn release(&self, id: Uuid) -> Result<Resources, ResourceManagerError> {
        let mut state = self.state.write().await;
        let result = state.reservations.remove(&id);
        if let Some(resources) = result {
            return Ok(resources);
        }

        tracing::error!(error = ?result, reservations=?state.reservations, "Failed to release resources.", );

        Err(ResourceManagerError::UnknownReservation)
    }
}
