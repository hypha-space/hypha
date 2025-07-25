use std::{
    cmp::Ordering, collections::{HashMap, HashSet}, future::Future, iter::Sum, ops::{Add, AddAssign, Sub, SubAssign}, sync::Arc
};

use hypha_messages::Requirement;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ComputeResources {
    pub cpu: f64,
    pub memory: f64,
    pub gpu: f64,
    pub storage: f64,
}

impl ComputeResources {
    pub fn new() -> Self {
        ComputeResources {
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

impl Default for ComputeResources {
    fn default() -> Self {
        Self::new()
    }
}

impl Sub for ComputeResources {
    type Output = ComputeResources;

    fn sub(self, other: ComputeResources) -> Self::Output {
        ComputeResources {
            cpu: self.cpu - other.cpu,
            memory: self.memory - other.memory,
            gpu: self.gpu - other.gpu,
            storage: self.storage - other.storage,
        }
    }
}

impl SubAssign for ComputeResources {
    fn sub_assign(&mut self, other: ComputeResources) {
        self.cpu -= other.cpu;
        self.memory -= other.memory;
        self.gpu -= other.gpu;
        self.storage -= other.storage;
    }
}

impl Add for ComputeResources {
    type Output = ComputeResources;

    fn add(self, other: ComputeResources) -> Self::Output {
        ComputeResources {
            cpu: self.cpu + other.cpu,
            memory: self.memory + other.memory,
            gpu: self.gpu + other.gpu,
            storage: self.storage + other.storage,
        }
    }
}

impl AddAssign for ComputeResources {
    fn add_assign(&mut self, other: ComputeResources) {
        self.cpu += other.cpu;
        self.memory += other.memory;
        self.gpu += other.gpu;
        self.storage += other.storage;
    }
}

impl PartialEq for ComputeResources {
    fn eq(&self, other: &Self) -> bool {
        self.cpu == other.cpu
            && self.memory == other.memory
            && self.gpu == other.gpu
            && self.storage == other.storage
    }
}

impl PartialOrd for ComputeResources {
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
                _ => Err(()),
            }
        })
        .ok()
    }
}

impl Sum for ComputeResources {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(ComputeResources::new(), |acc, x| acc + x)
    }
}

impl<'a> Sum<&'a ComputeResources> for ComputeResources {
    fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self {
        iter.fold(ComputeResources::new(), |acc, x| acc + *x)
    }
}

/// Extracts resource requirements from a worker specification
pub fn extract_compute_resource_requirements(requirements: &[Requirement]) -> ComputeResources {
    requirements
        .iter()
        .fold(ComputeResources::new(), |acc, req| match req {
            Requirement::Resource(hypha_messages::Resources::Cpu { min }) => {
                acc + ComputeResources::new().with_cpu(*min)
            }
            Requirement::Resource(hypha_messages::Resources::Gpu { min }) => {
                acc + ComputeResources::new().with_gpu(*min)
            }
            Requirement::Resource(hypha_messages::Resources::Memory { min }) => {
                acc + ComputeResources::new().with_memory(*min)
            }
            Requirement::Resource(hypha_messages::Resources::Storage { min }) => {
                acc + ComputeResources::new().with_storage(*min)
            }
            Requirement::Driver { .. } => {
                acc
            }
        })
}

/// Extracts resource requirements from a worker specification
pub fn extract_other_resource_requirements(requirements: &[Requirement]) -> Vec<String> {
    requirements
        .iter()
        .filter_map(|x| match x {
            Requirement::Resource(..) => {
                None
            },
            Requirement::Driver { kind } => {
                Some(kind.clone())
            }
        }).collect()
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
    fn compute_resources(&self) -> impl Future<Output = ComputeResources> + Send;
    fn other_resources(&self) -> impl Future<Output = Vec<String>> + Send;
    fn reserve(
        &self,
        compute_resources: ComputeResources,
        other_resources: Vec<String>,
    ) -> impl Future<Output = Result<Uuid, ResourceManagerError>> + Send;
    fn release(
        &self,
        id: Uuid,
    ) -> impl Future<Output = Result<ComputeResources, ResourceManagerError>> + Send;
}

#[derive(Debug)]
struct StaticResourceManagerState {
    compute_resources: ComputeResources,
    other_resources: Vec<String>,
    reservations: HashMap<Uuid, ComputeResources>,
}

#[derive(Debug, Clone)]
pub struct StaticResourceManager {
    state: Arc<RwLock<StaticResourceManagerState>>,
}

impl StaticResourceManager {
    pub fn new(compute_resources: ComputeResources, other_resources: Vec<String>) -> Self {
        StaticResourceManager {
            state: Arc::new(RwLock::new(StaticResourceManagerState {
                compute_resources,
                other_resources,
                reservations: HashMap::default(),
            })),
        }
    }
}

impl ResourceManager for StaticResourceManager {
    async fn compute_resources(&self) -> ComputeResources {
        let state = self.state.read().await;
        state.compute_resources - state.reservations.values().sum()
    }
    
    async fn other_resources(&self) -> Vec<String> {
        let state = self.state.read().await;
        state.other_resources.clone()
    }

    async fn reserve(&self, compute_resources: ComputeResources, other_resources: Vec<String>) -> Result<Uuid, ResourceManagerError> {
        let available = self.compute_resources().await;
        let hard_constrains = self.other_resources().await;
        tracing::info!("Reserving resources: {:?} of {:?} and {:?} of {:?}", compute_resources, available, other_resources, hard_constrains);
        
        let other_resources_set = HashSet::<&String>::from_iter(other_resources.iter());
        let driver_miss_match = HashSet::from_iter(hard_constrains.iter()).is_disjoint(&other_resources_set) ;

        if available >= compute_resources && !driver_miss_match {
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

    async fn release(&self, id: Uuid) -> Result<ComputeResources, ResourceManagerError> {
        let mut state = self.state.write().await;
        let result = state.reservations.remove(&id);
        if let Some(resources) = result {
            return Ok(resources);
        }

        tracing::error!(error = ?result, reservations=?state.reservations, "Failed to release resources.", );

        Err(ResourceManagerError::UnknownReservation)
    }
}
