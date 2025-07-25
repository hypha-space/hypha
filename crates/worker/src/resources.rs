use std::{
    cmp::Ordering,
    collections::HashMap,
    future::Future,
    iter::Sum,
    ops::{Add, AddAssign, Sub, SubAssign},
    sync::Arc,
};

use hypha_messages::Requirement;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Resources {
    pub cpu: f64,
    pub memory: f64,
    pub gpu: f64,
    pub storage: f64,
}

impl Resources {
    pub fn new() -> Self {
        Resources {
            cpu: 0.0,
            memory: 0.0,
            gpu: 0.0,
            storage: 0.0,
        }
    }

    pub fn with_cpu(mut self, cpu: f64) -> Self {
        self.cpu = cpu;
        self
    }

    pub fn with_memory(mut self, memory: f64) -> Self {
        self.memory = memory;
        self
    }

    pub fn with_gpu(mut self, gpu: f64) -> Self {
        self.gpu = gpu;
        self
    }

    pub fn with_storage(mut self, storage: f64) -> Self {
        self.storage = storage;
        self
    }
}

impl Sub for Resources {
    type Output = Resources;

    fn sub(self, other: Resources) -> Self::Output {
        Resources {
            cpu: self.cpu - other.cpu,
            memory: self.memory - other.memory,
            gpu: self.gpu - other.gpu,
            storage: self.storage - other.storage,
        }
    }
}

impl SubAssign for Resources {
    fn sub_assign(&mut self, other: Resources) {
        self.cpu -= other.cpu;
        self.memory -= other.memory;
        self.gpu -= other.gpu;
        self.storage -= other.storage;
    }
}

impl Add for Resources {
    type Output = Resources;

    fn add(self, other: Resources) -> Self::Output {
        Resources {
            cpu: self.cpu + other.cpu,
            memory: self.memory + other.memory,
            gpu: self.gpu + other.gpu,
            storage: self.storage + other.storage,
        }
    }
}

impl AddAssign for Resources {
    fn add_assign(&mut self, other: Resources) {
        self.cpu += other.cpu;
        self.memory += other.memory;
        self.gpu += other.gpu;
        self.storage += other.storage;
    }
}

impl PartialEq for Resources {
    fn eq(&self, other: &Self) -> bool {
        self.cpu == other.cpu
            && self.memory == other.memory
            && self.gpu == other.gpu
            && self.storage == other.storage
    }
}

impl PartialOrd for Resources {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        [
            (self.cpu, other.cpu),
            (self.memory, other.memory),
            (self.gpu, other.gpu),
            (self.storage, other.storage),
        ]
        .iter()
        .try_fold(Ordering::Equal, |acc, &(a, b)| {
            let field = a.partial_cmp(&b).ok_or(())?;

            match (acc, field) {
                (o1, o2) if o1 == o2 => Ok(o1),
                (Ordering::Equal, o) | (o, Ordering::Equal) => Ok(o),
                _ => return Err(()),
            }
        })
        .ok()
    }
}

impl Sum for Resources {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Resources::new(), |acc, x| acc + x)
    }
}

impl<'a> Sum<&'a Resources> for Resources {
    fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self {
        iter.fold(Resources::new(), |acc, x| acc + x.clone())
    }
}

/// Extracts resource requirements from a worker specification
pub fn extract_resource_requirements(requirements: &Vec<Requirement>) -> Resources {
    requirements
        .iter()
        .fold(Resources::new(), |acc, req| match req {
            Requirement::Resource(hypha_messages::Resources::Cpu { min }) => {
                acc + Resources::new().with_cpu(*min)
            }
            Requirement::Resource(hypha_messages::Resources::Gpu { min }) => {
                acc + Resources::new().with_gpu(*min)
            }
            Requirement::Resource(hypha_messages::Resources::Memory { min }) => {
                acc + Resources::new().with_memory(*min)
            }
            Requirement::Resource(hypha_messages::Resources::Storage { min }) => {
                acc + Resources::new().with_storage(*min)
            }
        })
}

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
    fn resources(&self) -> impl Future<Output = Resources> + Send;
    fn reserve(
        &self,
        resources: Resources,
    ) -> impl Future<Output = Result<Uuid, ResourceManagerError>> + Send;
    fn release(
        &self,
        id: Uuid,
    ) -> impl Future<Output = Result<Resources, ResourceManagerError>> + Send;
}

#[derive(Debug)]
struct StaticResourceManagerState {
    resources: Resources,
    reservations: HashMap<Uuid, Resources>,
}

#[derive(Debug, Clone)]
pub struct StaticResourceManager {
    state: Arc<RwLock<StaticResourceManagerState>>,
}

impl StaticResourceManager {
    pub fn new(resources: Resources) -> Self {
        StaticResourceManager {
            state: Arc::new(RwLock::new(StaticResourceManagerState {
                resources,
                reservations: HashMap::default(),
            })),
        }
    }
}

impl ResourceManager for StaticResourceManager {
    async fn resources(&self) -> Resources {
        let state = self.state.read().await;
        state.resources - state.reservations.values().sum()
    }

    async fn reserve(&self, resources: Resources) -> Result<Uuid, ResourceManagerError> {
        let available = self.resources().await;
        tracing::info!("Reserving resources: {:?} of {:?}", resources, available);

        if available >= resources {
            let mut state = self.state.write().await;
            // Re-check after acquiring write lock
            let current_available = state.resources - state.reservations.values().sum();
            if current_available >= resources {
                let id = Uuid::new_v4();
                state.reservations.insert(id, resources);
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
