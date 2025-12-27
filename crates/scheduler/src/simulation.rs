use itertools::Itertools;

pub trait Simulation {
    fn project(
        progress: &[u64],
        batch_sizes: &[u32],
        statistics: Vec<u64>,
        target: u32,
        steps_cap: u32,
    ) -> (u64, i32, Vec<u32>, bool);
}

pub struct BasicSimulation {}

impl Simulation for BasicSimulation {
    fn project(
        progress: &[u64],
        batch_sizes: &[u32],
        statistics: Vec<u64>,
        data_points_left: u32,
        steps_cap: u32,
    ) -> (u64, i32, Vec<u32>, bool) {
        // NOTE: `time` and `next_update` are u64 to represent time in ms.
        // Counters remain u32 to reflect counts/steps within practical bounds.
        let mut updates = vec![0u32; batch_sizes.len()];
        let mut next_update: Vec<u64> = progress
            .iter()
            .zip(statistics.iter())
            .map(|(a, b)| a + b)
            .collect();
        let mut time: u64 = 0;
        let mut to_go = data_points_left as i32;
        let mut capped = false;
        while to_go > 0 {
            let min_set = next_update
                .iter()
                .zip(0..batch_sizes.len())
                .min_set_by_key(|&(k, _)| k);

            let next_event_time = *min_set[0].0;
            time = next_event_time;

            let mut max_steps_reached = false;
            let min_indices: Vec<usize> = min_set.iter().map(|(_, idx)| *idx).collect();
            for idx in min_indices.iter() {
                let idx = *idx;
                to_go -= batch_sizes[idx] as i32;
                updates[idx] = updates[idx].saturating_add(1);

                if updates[idx] >= steps_cap {
                    max_steps_reached = true;
                }
                next_update[idx] = next_update[idx].saturating_add(statistics[idx]);
            }

            if max_steps_reached {
                capped = true;
                break;
            }
        }
        (time, to_go, updates, capped)
    }
}

#[cfg(test)]
mod tests {
    use crate::simulation::{BasicSimulation, Simulation};

    #[test]
    fn test_simulation_hits_steps_cap() {
        let batch_sizes = [10, 5, 1];
        let statistics = vec![20, 30, 50];
        let (time, done, updates, capped) =
            BasicSimulation::project(&[0; 3], &batch_sizes, statistics, 9999, 3);

        // Simulation trace:
        // ...
        // t=60: Worker 0 updates to 3. Stream 1 updates to 2.
        // `max_steps_reached` becomes true. Loop breaks.

        assert_eq!(time, 60, "Stops at t=60 when S0 hits 3 steps");
        assert_eq!(done, 9999 - 41, "Done count at t=60");
        assert_eq!(updates, vec![3, 2, 1], "S0 hits the cap of 3");
        assert_eq!(capped, true, "Limit reached")
    }

    #[test]
    fn test_simulation_hits_update_target() {
        let batch_sizes = [10, 5, 1];
        let statistics = vec![20, 30, 50];
        let (time, done, updates, capped) =
            BasicSimulation::project(&[0; 3], &batch_sizes, statistics, 50, 999);

        // Simulation trace:
        // ...
        // t=60: done=41
        // t=80: S0 fires. done=41+10=51.
        // Loop condition (target - done) > 0 -> (50 - 51) > 0 is false.
        // Loop terminates *before* next event.

        assert_eq!(time, 80, "Stops at t=80 when done 0");
        assert_eq!(done, -1, "Final done count");
        assert_eq!(updates, vec![4, 2, 1], "Updates at t=80");
        assert_eq!(capped, false, "Limit not reached")
    }
}
