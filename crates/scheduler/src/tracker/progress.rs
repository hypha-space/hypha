use libp2p::PeerId;
use tokio::time::Instant;

use crate::{
    statistics::RuntimeStatistic,
    tracker::worker::{WorkerTracker, WorkerTrackerError},
};

pub struct ProgressTracker<T>
where
    T: RuntimeStatistic,
{
    counter: u32,
    parameter_server: PeerId,
    update_target: u32,
    round_start_instant: Instant,
    update_epochs: u32,
    update_counter: u32,
    pub worker_tracker: WorkerTracker<T>,
}

impl<T> ProgressTracker<T>
where
    T: RuntimeStatistic,
{
    pub fn new(parameter_server: PeerId, update_target: u32, update_epochs: u32) -> Self {
        ProgressTracker {
            counter: update_target,
            parameter_server,
            update_target,
            round_start_instant: tokio::time::Instant::now(),
            update_epochs,
            update_counter: 0,
            worker_tracker: WorkerTracker::<T>::new(),
        }
    }

    pub fn update_parameter_server(&mut self, new_paramter_server: PeerId) {
        self.parameter_server = new_paramter_server;
    }

    pub fn update(&mut self, id: &PeerId, count: u32) -> Result<(), WorkerTrackerError> {
        self.counter = self.counter.saturating_sub(count);
        self.worker_tracker
            .update(id, self.round_start_instant.elapsed().as_millis() as u64)?;
        Ok(())
    }

    pub fn next_round(&mut self) {
        self.counter = self.update_target;
        self.round_start_instant = Instant::now();
        self.update_counter += 1;
        self.worker_tracker.new_round();
    }

    pub fn count(&self) -> u32 {
        self.counter
    }

    pub fn round(&self) -> u32 {
        self.update_counter
    }

    pub fn training_finished(&self) -> bool {
        self.update_counter == self.update_epochs
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use libp2p::PeerId;

    use crate::{
        statistics::RunningMean,
        tracker::{
            progress::ProgressTracker,
            worker::{State, WorkerTracker},
        },
    };

    #[test]
    fn worker_position() {
        let mut tracker = WorkerTracker::<RunningMean>::new();
        let peer_id = PeerId::random();
        tracker.add_worker(PeerId::random(), 12);
        tracker.add_worker(peer_id, 2);
        tracker.add_worker(PeerId::random(), 12);
        let position = tracker.worker_position(&peer_id).expect("Found");
        assert_eq!(position, 1);
        tracker.remove_worker(&peer_id).expect("Done");
        assert!(tracker.worker_position(&PeerId::random()).is_err());
    }

    #[tokio::test]
    async fn peer_info() {
        let mut tracker = ProgressTracker::<RunningMean>::new(PeerId::random(), 1024, 1);
        tokio::time::pause();
        let p1 = PeerId::random();
        tracker.worker_tracker.add_worker(p1, 2);
        let p2 = PeerId::random();
        tracker.worker_tracker.add_worker(p2, 12);
        assert_eq!(tracker.worker_tracker.worker(), vec![p1, p2]);

        assert_eq!(tracker.worker_tracker.batch_sizes(), vec![2, 12]);
        tokio::time::advance(Duration::from_millis(12)).await;
        tokio::time::resume();
        tracker.update(&p1, 2).expect("p1 Update");
        tokio::time::pause();

        assert_eq!(tracker.count(), 1022);
        assert!(tracker.worker_tracker.last_updates()[0] >= 10);
        assert_eq!(tracker.worker_tracker.last_updates()[1], 0);

        tokio::time::advance(Duration::from_millis(38)).await;
        tokio::time::resume();
        tracker.update(&p2, 12).expect("p2 Update");
        assert_eq!(tracker.worker_tracker.estimates(), vec![12, 50]);

        tracker.next_round();
        assert_eq!(tracker.worker_tracker.last_updates(), vec![0, 0]);
        assert_eq!(tracker.count(), 1024);
        assert_eq!(tracker.training_finished(), true);

        tracker.worker_tracker.done();
        assert_eq!(tracker.worker_tracker.states(), vec![State::Done; 2]);
    }

    #[test]
    fn peer_state() {
        let mut tracker = ProgressTracker::<RunningMean>::new(PeerId::random(), 1024, 1);
        let p1 = PeerId::random();
        tracker.worker_tracker.add_worker(p1, 2);

        assert_eq!(tracker.worker_tracker.worker_state(&p1), State::Training);
        tracker
            .worker_tracker
            .update_worker_state(&p1, State::UpdateScheduled);
        assert_eq!(
            tracker.worker_tracker.worker_state(&p1),
            State::UpdateScheduled
        );
        assert_eq!(
            tracker.worker_tracker.states(),
            vec![State::UpdateScheduled]
        );
    }

    #[test]
    fn parameter_server_update() {
        let mut tracker = ProgressTracker::<RunningMean>::new(PeerId::random(), 1024, 1);
        let ps = PeerId::random();
        tracker.update_parameter_server(ps);
        assert_eq!(tracker.parameter_server, ps)
    }
}
