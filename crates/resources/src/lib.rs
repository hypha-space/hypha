use std::{
    cmp::Ordering,
    iter::Sum,
    ops::{Add, AddAssign, Sub, SubAssign},
};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Resources {
    gpu: f64,
    cpu: f64,
    storage: f64,
    memory: f64,
}

impl Resources {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cpu(&self) -> f64 {
        self.cpu
    }

    pub fn memory(&self) -> f64 {
        self.memory
    }

    pub fn gpu(&self) -> f64 {
        self.gpu
    }

    pub fn storage(&self) -> f64 {
        self.storage
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

impl Default for Resources {
    fn default() -> Self {
        Self {
            gpu: 0.0,
            cpu: 0.0,
            storage: 0.0,
            memory: 0.0,
        }
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
                _ => Err(()),
            }
        })
        .ok()
    }
}

impl Sum for Resources {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Resources::default(), |acc, x| acc + x)
    }
}

impl<'a> Sum<&'a Resources> for Resources {
    fn sum<I: Iterator<Item = &'a Self>>(iter: I) -> Self {
        iter.fold(Resources::default(), |acc, x| acc + *x)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WeightedResourceEvaluator {
    pub cpu: f64,
    pub gpu: f64,
    pub memory: f64,
    pub storage: f64,
}

impl WeightedResourceEvaluator {
    pub fn evaluate(&self, price: f64, resources: &Resources) -> f64 {
        let weight_units = self.gpu * resources.gpu()
            + self.cpu * resources.cpu()
            + self.memory * resources.memory()
            + self.storage * resources.storage();

        if weight_units > 0.0 {
            return price / weight_units;
        }

        0.0
    }
}

impl Default for WeightedResourceEvaluator {
    fn default() -> Self {
        Self {
            cpu: 1.0,
            gpu: 25.0,
            memory: 0.1,
            storage: 0.01,
        }
    }
}

pub trait ResourceEvaluator {
    fn score(&self, price: f64, resources: &Resources) -> f64;
}

impl ResourceEvaluator for WeightedResourceEvaluator {
    fn score(&self, price: f64, resources: &Resources) -> f64 {
        self.evaluate(price, resources)
    }
}
