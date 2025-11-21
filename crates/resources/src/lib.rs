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

pub trait ResourceEvaluator {
    fn score(&self, price: f64, resources: &Resources) -> f64;
}

#[derive(Debug, Clone, Copy)]
pub struct WeightedResourceEvaluator {
    pub cpu: f64,
    pub gpu: f64,
    pub memory: f64,
    pub storage: f64,
}

impl ResourceEvaluator for WeightedResourceEvaluator {
    fn score(&self, price: f64, resources: &Resources) -> f64 {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_resources() -> Resources {
        Resources::new()
            .with_cpu(4.0)
            .with_memory(32.0)
            .with_gpu(1.0)
            .with_storage(250.0)
    }

    mod resources {
        use super::*;

        #[test]
        fn default_is_zeroed() {
            let resources = Resources::new();

            assert_eq!(resources.cpu(), 0.0);
            assert_eq!(resources.memory(), 0.0);
            assert_eq!(resources.gpu(), 0.0);
            assert_eq!(resources.storage(), 0.0);
        }

        mod setters {
            use super::*;

            #[test]
            fn with_cpu_updates_cpu_only() {
                let resources = Resources::new().with_cpu(4.5);

                assert_eq!(resources.cpu(), 4.5);
                assert_eq!(resources.memory(), 0.0);
                assert_eq!(resources.gpu(), 0.0);
                assert_eq!(resources.storage(), 0.0);
            }

            #[test]
            fn with_memory_updates_memory_only() {
                let resources = Resources::new().with_memory(64.0);

                assert_eq!(resources.cpu(), 0.0);
                assert_eq!(resources.memory(), 64.0);
                assert_eq!(resources.gpu(), 0.0);
                assert_eq!(resources.storage(), 0.0);
            }

            #[test]
            fn with_gpu_updates_gpu_only() {
                let resources = Resources::new().with_gpu(2.0);

                assert_eq!(resources.cpu(), 0.0);
                assert_eq!(resources.memory(), 0.0);
                assert_eq!(resources.gpu(), 2.0);
                assert_eq!(resources.storage(), 0.0);
            }

            #[test]
            fn with_storage_updates_storage_only() {
                let resources = Resources::new().with_storage(512.0);

                assert_eq!(resources.cpu(), 0.0);
                assert_eq!(resources.memory(), 0.0);
                assert_eq!(resources.gpu(), 0.0);
                assert_eq!(resources.storage(), 512.0);
            }
        }

        mod arithmetic {
            use super::*;

            #[test]
            fn add_combines_each_field() {
                let base = sample_resources();
                let increment = Resources::new()
                    .with_cpu(1.0)
                    .with_memory(2.0)
                    .with_gpu(3.0)
                    .with_storage(4.0);

                let summed = base + increment;
                assert_eq!(summed.cpu(), 5.0);
                assert_eq!(summed.memory(), 34.0);
                assert_eq!(summed.gpu(), 4.0);
                assert_eq!(summed.storage(), 254.0);
            }

            #[test]
            fn add_assign_combines_each_field() {
                let base = sample_resources();
                let increment = Resources::new()
                    .with_cpu(1.0)
                    .with_memory(2.0)
                    .with_gpu(3.0)
                    .with_storage(4.0);

                let mut assign_target = base;
                assign_target += increment;
                assert_eq!(assign_target.cpu(), 5.0);
                assert_eq!(assign_target.memory(), 34.0);
                assert_eq!(assign_target.gpu(), 4.0);
                assert_eq!(assign_target.storage(), 254.0);
            }

            #[test]
            fn sub_reduces_each_field() {
                let base = sample_resources();
                let decrement = Resources::new()
                    .with_cpu(2.0)
                    .with_memory(8.0)
                    .with_gpu(0.25)
                    .with_storage(50.0);

                let diff = base - decrement;
                assert_eq!(diff.cpu(), 2.0);
                assert_eq!(diff.memory(), 24.0);
                assert!((diff.gpu() - 0.75).abs() < f64::EPSILON);
                assert_eq!(diff.storage(), 200.0);
            }

            #[test]
            fn sub_assign_reduces_each_field() {
                let base = sample_resources();
                let decrement = Resources::new()
                    .with_cpu(2.0)
                    .with_memory(8.0)
                    .with_gpu(0.25)
                    .with_storage(50.0);

                let mut assign_target = base;
                assign_target -= decrement;
                assert_eq!(assign_target.cpu(), 2.0);
                assert_eq!(assign_target.memory(), 24.0);
                assert!((assign_target.gpu() - 0.75).abs() < f64::EPSILON);
                assert_eq!(assign_target.storage(), 200.0);
            }
        }

        mod comparisons {
            use std::cmp::Ordering;

            use super::*;

            #[test]
            fn partial_eq_respects_each_field() {
                let base = sample_resources();
                let mut changed = base;
                changed = changed.with_cpu(5.0);

                assert_eq!(base, sample_resources());
                assert_ne!(base, changed);
            }

            #[test]
            fn partial_cmp_equal_returns_equal() {
                let base = sample_resources();

                assert_eq!(base.partial_cmp(&base), Some(Ordering::Equal));
            }

            #[test]
            fn partial_cmp_smaller_returns_less() {
                let base = sample_resources();
                let smaller = Resources::new()
                    .with_cpu(3.0)
                    .with_memory(16.0)
                    .with_gpu(0.5)
                    .with_storage(125.0);

                assert_eq!(smaller.partial_cmp(&base), Some(Ordering::Less));
            }

            #[test]
            fn partial_cmp_larger_returns_greater() {
                let base = sample_resources();
                let larger = Resources::new()
                    .with_cpu(5.0)
                    .with_memory(64.0)
                    .with_gpu(2.0)
                    .with_storage(500.0);

                assert_eq!(larger.partial_cmp(&base), Some(Ordering::Greater));
            }

            #[test]
            fn partial_cmp_conflicting_returns_none() {
                let base = sample_resources();
                let conflicting = Resources::new()
                    .with_cpu(5.0)
                    .with_memory(16.0)
                    .with_gpu(2.0)
                    .with_storage(125.0);

                assert_eq!(base.partial_cmp(&conflicting), None);
            }

            #[test]
            fn partial_cmp_with_nan_returns_none() {
                let base = sample_resources();
                let mut with_nan = base;
                with_nan = with_nan.with_gpu(f64::NAN);

                assert_eq!(with_nan.partial_cmp(&base), None);
            }
        }

        mod iteration {
            use super::*;

            #[test]
            fn sum_accumulates_owned_and_borrowed_iterators() {
                let resources = [
                    Resources::new().with_cpu(1.0).with_memory(2.0),
                    Resources::new()
                        .with_cpu(3.0)
                        .with_memory(4.0)
                        .with_gpu(1.0)
                        .with_storage(10.0),
                ];

                let owned_sum: Resources = resources.into_iter().sum();
                assert_eq!(owned_sum.cpu(), 4.0);
                assert_eq!(owned_sum.memory(), 6.0);
                assert_eq!(owned_sum.gpu(), 1.0);
                assert_eq!(owned_sum.storage(), 10.0);

                let borrowed_sum: Resources = resources.iter().sum();
                assert_eq!(borrowed_sum.cpu(), 4.0);
                assert_eq!(borrowed_sum.memory(), 6.0);
                assert_eq!(borrowed_sum.gpu(), 1.0);
                assert_eq!(borrowed_sum.storage(), 10.0);
            }
        }
    }

    mod weighted_evaluator {
        use super::*;

        #[test]
        fn scrores_price_per_weighted_unit() {
            let evaluator = WeightedResourceEvaluator {
                cpu: 2.0,
                gpu: 1.0,
                memory: 0.5,
                storage: 0.0,
            };
            let resources = Resources::new()
                .with_cpu(4.0)
                .with_gpu(2.0)
                .with_memory(8.0);

            let weight_units = 2.0 * 4.0 + 1.0 * 2.0 + 0.5 * 8.0;
            let expected = 10.0 / weight_units;
            assert_eq!(evaluator.score(10.0, &resources), expected);
        }

        #[test]
        fn zero_or_negative_weight_units_returns_zero() {
            let evaluator = WeightedResourceEvaluator {
                cpu: 0.0,
                gpu: 0.0,
                memory: 0.0,
                storage: 0.0,
            };
            let resources = sample_resources();
            assert_eq!(evaluator.score(10.0, &resources), 0.0);

            let evaluator = WeightedResourceEvaluator {
                cpu: -1.0,
                gpu: 0.0,
                memory: 0.0,
                storage: 0.0,
            };
            assert_eq!(evaluator.score(10.0, &resources), 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_resources() -> Resources {
        Resources::new()
            .with_cpu(4.0)
            .with_memory(32.0)
            .with_gpu(1.0)
            .with_storage(250.0)
    }

    mod resources {
        use super::*;

        #[test]
        fn default_is_zeroed() {
            let resources = Resources::new();

            assert_eq!(resources.cpu(), 0.0);
            assert_eq!(resources.memory(), 0.0);
            assert_eq!(resources.gpu(), 0.0);
            assert_eq!(resources.storage(), 0.0);
        }

        mod setters {
            use super::*;

            #[test]
            fn with_cpu_updates_cpu_only() {
                let resources = Resources::new().with_cpu(4.5);

                assert_eq!(resources.cpu(), 4.5);
                assert_eq!(resources.memory(), 0.0);
                assert_eq!(resources.gpu(), 0.0);
                assert_eq!(resources.storage(), 0.0);
            }

            #[test]
            fn with_memory_updates_memory_only() {
                let resources = Resources::new().with_memory(64.0);

                assert_eq!(resources.cpu(), 0.0);
                assert_eq!(resources.memory(), 64.0);
                assert_eq!(resources.gpu(), 0.0);
                assert_eq!(resources.storage(), 0.0);
            }

            #[test]
            fn with_gpu_updates_gpu_only() {
                let resources = Resources::new().with_gpu(2.0);

                assert_eq!(resources.cpu(), 0.0);
                assert_eq!(resources.memory(), 0.0);
                assert_eq!(resources.gpu(), 2.0);
                assert_eq!(resources.storage(), 0.0);
            }

            #[test]
            fn with_storage_updates_storage_only() {
                let resources = Resources::new().with_storage(512.0);

                assert_eq!(resources.cpu(), 0.0);
                assert_eq!(resources.memory(), 0.0);
                assert_eq!(resources.gpu(), 0.0);
                assert_eq!(resources.storage(), 512.0);
            }
        }

        mod arithmetic {
            use super::*;

            #[test]
            fn add_combines_each_field() {
                let base = sample_resources();
                let increment = Resources::new()
                    .with_cpu(1.0)
                    .with_memory(2.0)
                    .with_gpu(3.0)
                    .with_storage(4.0);

                let summed = base + increment;
                assert_eq!(summed.cpu(), 5.0);
                assert_eq!(summed.memory(), 34.0);
                assert_eq!(summed.gpu(), 4.0);
                assert_eq!(summed.storage(), 254.0);
            }

            #[test]
            fn add_assign_combines_each_field() {
                let base = sample_resources();
                let increment = Resources::new()
                    .with_cpu(1.0)
                    .with_memory(2.0)
                    .with_gpu(3.0)
                    .with_storage(4.0);

                let mut assign_target = base;
                assign_target += increment;
                assert_eq!(assign_target.cpu(), 5.0);
                assert_eq!(assign_target.memory(), 34.0);
                assert_eq!(assign_target.gpu(), 4.0);
                assert_eq!(assign_target.storage(), 254.0);
            }

            #[test]
            fn sub_reduces_each_field() {
                let base = sample_resources();
                let decrement = Resources::new()
                    .with_cpu(2.0)
                    .with_memory(8.0)
                    .with_gpu(0.25)
                    .with_storage(50.0);

                let diff = base - decrement;
                assert_eq!(diff.cpu(), 2.0);
                assert_eq!(diff.memory(), 24.0);
                assert!((diff.gpu() - 0.75).abs() < f64::EPSILON);
                assert_eq!(diff.storage(), 200.0);
            }

            #[test]
            fn sub_assign_reduces_each_field() {
                let base = sample_resources();
                let decrement = Resources::new()
                    .with_cpu(2.0)
                    .with_memory(8.0)
                    .with_gpu(0.25)
                    .with_storage(50.0);

                let mut assign_target = base;
                assign_target -= decrement;
                assert_eq!(assign_target.cpu(), 2.0);
                assert_eq!(assign_target.memory(), 24.0);
                assert!((assign_target.gpu() - 0.75).abs() < f64::EPSILON);
                assert_eq!(assign_target.storage(), 200.0);
            }
        }

        mod comparisons {
            use std::cmp::Ordering;

            use super::*;

            #[test]
            fn partial_eq_respects_each_field() {
                let base = sample_resources();
                let mut changed = base;
                changed = changed.with_cpu(5.0);

                assert_eq!(base, sample_resources());
                assert_ne!(base, changed);
            }

            #[test]
            fn partial_cmp_equal_returns_equal() {
                let base = sample_resources();

                assert_eq!(base.partial_cmp(&base), Some(Ordering::Equal));
            }

            #[test]
            fn partial_cmp_smaller_returns_less() {
                let base = sample_resources();
                let smaller = Resources::new()
                    .with_cpu(3.0)
                    .with_memory(16.0)
                    .with_gpu(0.5)
                    .with_storage(125.0);

                assert_eq!(smaller.partial_cmp(&base), Some(Ordering::Less));
            }

            #[test]
            fn partial_cmp_larger_returns_greater() {
                let base = sample_resources();
                let larger = Resources::new()
                    .with_cpu(5.0)
                    .with_memory(64.0)
                    .with_gpu(2.0)
                    .with_storage(500.0);

                assert_eq!(larger.partial_cmp(&base), Some(Ordering::Greater));
            }

            #[test]
            fn partial_cmp_conflicting_returns_none() {
                let base = sample_resources();
                let conflicting = Resources::new()
                    .with_cpu(5.0)
                    .with_memory(16.0)
                    .with_gpu(2.0)
                    .with_storage(125.0);

                assert_eq!(base.partial_cmp(&conflicting), None);
            }

            #[test]
            fn partial_cmp_with_nan_returns_none() {
                let base = sample_resources();
                let mut with_nan = base;
                with_nan = with_nan.with_gpu(f64::NAN);

                assert_eq!(with_nan.partial_cmp(&base), None);
            }
        }

        mod iteration {
            use super::*;

            #[test]
            fn sum_accumulates_owned_and_borrowed_iterators() {
                let resources = [
                    Resources::new().with_cpu(1.0).with_memory(2.0),
                    Resources::new()
                        .with_cpu(3.0)
                        .with_memory(4.0)
                        .with_gpu(1.0)
                        .with_storage(10.0),
                ];

                let owned_sum: Resources = resources.into_iter().sum();
                assert_eq!(owned_sum.cpu(), 4.0);
                assert_eq!(owned_sum.memory(), 6.0);
                assert_eq!(owned_sum.gpu(), 1.0);
                assert_eq!(owned_sum.storage(), 10.0);

                let borrowed_sum: Resources = resources.iter().sum();
                assert_eq!(borrowed_sum.cpu(), 4.0);
                assert_eq!(borrowed_sum.memory(), 6.0);
                assert_eq!(borrowed_sum.gpu(), 1.0);
                assert_eq!(borrowed_sum.storage(), 10.0);
            }
        }
    }

    mod weighted_evaluator {
        use super::*;

        #[test]
        fn scrores_price_per_weighted_unit() {
            let evaluator = WeightedResourceEvaluator {
                cpu: 2.0,
                gpu: 1.0,
                memory: 0.5,
                storage: 0.0,
            };
            let resources = Resources::new()
                .with_cpu(4.0)
                .with_gpu(2.0)
                .with_memory(8.0);

            let weight_units = 2.0 * 4.0 + 1.0 * 2.0 + 0.5 * 8.0;
            let expected = 10.0 / weight_units;
            assert_eq!(evaluator.score(10.0, &resources), expected);
        }

        #[test]
        fn zero_or_negative_weight_units_returns_zero() {
            let evaluator = WeightedResourceEvaluator {
                cpu: 0.0,
                gpu: 0.0,
                memory: 0.0,
                storage: 0.0,
            };
            let resources = sample_resources();
            assert_eq!(evaluator.score(10.0, &resources), 0.0);

            let evaluator = WeightedResourceEvaluator {
                cpu: -1.0,
                gpu: 0.0,
                memory: 0.0,
                storage: 0.0,
            };
            assert_eq!(evaluator.score(10.0, &resources), 0.0);
        }
    }
}
