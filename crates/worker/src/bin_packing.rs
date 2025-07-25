// use hypha_messages::{Driver, WorkerRequest};
// use libp2p::PeerId;
// use ordered_float::OrderedFloat;

// use crate::resource_observer::ResourceBundle;

// #[derive(Clone, Debug)]
// pub struct ExtraRequirements {
//     pub driver: Option<Driver>,
// }

// impl From<&hypha_messages::WorkerSpec> for ExtraRequirements {
//     fn from(spec: &hypha_messages::WorkerSpec) -> Self {
//         use hypha_messages::Requirement;

//         let driver = spec.requirements.iter().find_map(|r| {
//             if let Requirement::Driver(d) = r {
//                 Some(d.clone())
//             } else {
//                 None
//             }
//         });

//         ExtraRequirements { driver }
//     }
// }

// impl ExtraRequirements {
//     /// Check if a worker with given capabilities can satisfy these requirements
//     pub fn can_be_satisfied_by(&self, worker_capabilities: &ExtraRequirements) -> bool {
//         // For now, we only check driver compatibility
//         // In the future, this could be expanded to check for other non-numeric requirements
//         match (&self.driver, &worker_capabilities.driver) {
//             (None, _) => true,        // No driver requirement
//             (Some(_), None) => false, // Driver required but not available
//             (Some(required), Some(available)) => {
//                 // For now, simple type matching - in reality this would be more sophisticated
//                 std::mem::discriminant(required) == std::mem::discriminant(available)
//             }
//         }
//     }

//     /// Check if there are any non-numeric requirements that need to be validated
//     pub fn has_requirements(&self) -> bool {
//         self.driver.is_some()
//     }
// }

// #[derive(Debug, Clone, Copy, PartialEq)]
// pub enum PackingStrategy {
//     /// First-Fit Decreasing (battle-tested O(n log n) algorithm)
//     Default,
// }

// // NOTE: Main entry point for bin packing with strategy selection
// pub fn pack_requests_with_strategy(
//     capacity: &ResourceBundle,
//     requests: &[(PeerId, WorkerRequest)],
//     strategy: PackingStrategy,
//     value_fn: impl Fn(&WorkerRequest) -> f64,
// ) -> Vec<(PeerId, WorkerRequest)> {
//     match strategy {
//         PackingStrategy::Default => default_packing(capacity, requests, value_fn),
//     }
// }

// /// Default packing: First Fit Decreasing - Sort by value density, then pack greedily
// pub fn default_packing(
//     capacity: &ResourceBundle,
//     requests: &[(PeerId, WorkerRequest)],
//     value_fn: impl Fn(&WorkerRequest) -> f64,
// ) -> Vec<(PeerId, WorkerRequest)> {
//     // Sort by value density (value per total resource unit)
//     let mut sorted_requests: Vec<_> = requests
//         .iter()
//         .map(|(peer_id, request)| {
//             let resources: ResourceBundle = (&request.spec).into();
//             let value = value_fn(request);
//             let density = if resources.total_units() > 0.0 {
//                 value / resources.total_units()
//             } else {
//                 0.0
//             };
//             (
//                 (*peer_id, request.clone()),
//                 OrderedFloat(density),
//                 resources,
//             )
//         })
//         .collect();

//     sorted_requests.sort_by_key(|(_, density, _)| std::cmp::Reverse(*density));

//     // Greedy packing
//     let mut packed = Vec::new();
//     let mut remaining_capacity = capacity.clone();

//     for ((peer_id, request), _, resources) in sorted_requests {
//         if resources.fits_within(&remaining_capacity) {
//             remaining_capacity = remaining_capacity.subtract(&resources);
//             packed.push((peer_id, request));
//         }
//     }

//     packed
// }

// /// Calculate profitability density for sorting
// pub fn calculate_profitability_density(request: &WorkerRequest) -> f64 {
//     let resources: ResourceBundle = (&request.spec).into();
//     let total_resources = resources.total_units();

//     if total_resources > 0.0 {
//         request.bid / total_resources
//     } else {
//         0.0
//     }
// }

// impl From<&hypha_messages::WorkerSpec> for ResourceBundle {
//     fn from(spec: &hypha_messages::WorkerSpec) -> Self {
//         use hypha_messages::{Requirement, Resource};

//         let mut bundle = ResourceBundle::new(0.0, 0.0, 0.0, 0.0);

//         for requirement in &spec.requirements {
//             if let Requirement::Resource(resource) = requirement {
//                 match resource {
//                     Resource::Cpu { min } => bundle.cpu += min,
//                     Resource::Memory { min } => bundle.memory += min,
//                     Resource::Gpu { min } => bundle.gpu += min,
//                     Resource::Disk { min } => bundle.disk += min,
//                 }
//             }
//         }

//         // If no resource requirements specified, use defaults
//         if bundle.total_units() == 0.0 {
//             bundle = ResourceBundle::new(1.0, 2.0, 0.0, 1.0); // Default: 1 CPU, 2GB RAM, 1GB disk
//         }

//         bundle
//     }
// }

// /// Check if a request fits within remaining capacity
// pub fn request_fits_in_capacity(
//     spec: &hypha_messages::WorkerSpec,
//     capacity: &ResourceBundle,
// ) -> bool {
//     let required: ResourceBundle = spec.into();
//     required.fits_within(capacity)
// }

// /// Utility function for combining multiple bin packing results
// pub fn combine_packing_results(
//     results: Vec<Vec<(PeerId, WorkerRequest)>>,
//     capacity: &ResourceBundle,
// ) -> Vec<(PeerId, WorkerRequest)> {
//     let mut combined = Vec::new();
//     let mut remaining_capacity = capacity.clone();

//     for result in results {
//         for (peer_id, request) in result {
//             let resources: ResourceBundle = (&request.spec).into();
//             if resources.fits_within(&remaining_capacity) {
//                 remaining_capacity = remaining_capacity.subtract(&resources);
//                 combined.push((peer_id, request));
//             }
//         }
//     }

//     combined
// }

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use chrono::Utc;
//     use hypha_messages::{Requirement, Resource, WorkerSpec};
//     use uuid::Uuid;

//     fn create_test_request(cpu: f64, memory: f64, bid: f64) -> WorkerRequest {
//         let spec = WorkerSpec {
//             requirements: vec![
//                 Requirement::Resource(Resource::Cpu { min: cpu }),
//                 Requirement::Resource(Resource::Memory { min: memory }),
//             ],
//         };

//         WorkerRequest {
//             id: Uuid::new_v4(),
//             spec,
//             timeout: Utc::now() + chrono::Duration::hours(1),
//             bid,
//         }
//     }

//     #[test]
//     fn test_default_packing() {
//         let capacity = ResourceBundle::new(10.0, 20.0, 0.0, 10.0);
//         let peer_id = "12D3KooWTest".parse().unwrap();

//         let requests = vec![
//             (peer_id, create_test_request(2.0, 4.0, 10.0)), // High value density
//             (peer_id, create_test_request(8.0, 16.0, 20.0)), // Lower value density
//             (peer_id, create_test_request(1.0, 2.0, 8.0)),  // Highest value density
//         ];

//         let result = default_packing(&capacity, &requests, |req| req.bid);

//         // Should select items with highest value density first
//         assert_eq!(result.len(), 2); // Only 2 should fit due to resource constraints
//         assert_eq!(result[0].1.bid, 8.0); // Highest density first
//     }

//     #[test]
//     fn test_capacity_constraints() {
//         let capacity = ResourceBundle::new(5.0, 10.0, 0.0, 5.0);
//         let peer_id = "12D3KooWTest".parse().unwrap();

//         let requests = vec![
//             (peer_id, create_test_request(3.0, 6.0, 15.0)),
//             (peer_id, create_test_request(3.0, 6.0, 12.0)), // Won't fit
//         ];

//         let result = default_packing(&capacity, &requests, |req| req.bid);

//         // Only first request should fit
//         assert_eq!(result.len(), 1);
//         assert_eq!(result[0].1.bid, 15.0);
//     }
// }
