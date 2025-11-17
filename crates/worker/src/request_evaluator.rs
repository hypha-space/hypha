use hypha_messages::request_worker::Request;

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
        let resources = &request.spec.resources;

        let weight_units = self.gpu * resources.gpu()
            + self.cpu * resources.cpu()
            + self.memory * resources.memory()
            + self.storage * resources.storage();

        if weight_units > 0.0 {
            return request.bid / weight_units;
        }

        0.0
    }
}
