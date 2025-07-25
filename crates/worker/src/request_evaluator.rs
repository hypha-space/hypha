use hypha_messages::request_worker::Request;

use crate::resources::{extract_compute_resource_requirements};

pub trait RequestEvaluator {
    fn score(&self, request: &Request) -> f64;
}

pub struct WeightedResourceRequestEvaluator {
    pub cpu: f64,
    pub gpu: f64,
    pub memory: f64,
    pub storage: f64,
}

impl Default for WeightedResourceRequestEvaluator {
    fn default() -> Self {
        Self {
            cpu: 1.0,
            gpu: 25.0,
            memory: 0.1,
            storage: 0.01,
        }
    }
}

impl RequestEvaluator for WeightedResourceRequestEvaluator {
    fn score(&self, request: &Request) -> f64 {
        let compute_resources = extract_compute_resource_requirements(&request.spec.requirements);

        let weight_units = self.gpu * compute_resources.gpu
            + self.cpu * compute_resources.cpu
            + self.memory * compute_resources.memory
            + self.storage * compute_resources.storage;

        if weight_units > 0.0 {
            return request.bid / weight_units;
        }

        0.0
    }
}
