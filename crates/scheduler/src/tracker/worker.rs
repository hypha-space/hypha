use libp2p::PeerId;
use thiserror::Error;

use crate::statistics::RuntimeStatistic;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    Training,
    UpdateScheduled,
    Updating,
    Done,
}

#[derive(Debug, Error)]
pub enum WorkerTrackerError {
    #[error("Peer not found")]
    Peer,
}

pub struct WorkerTracker<T>
where
    T: RuntimeStatistic,
{
    pub(crate) last_update: Vec<u64>,
    peer_ids: Vec<PeerId>,
    batch_sizes: Vec<u32>,
    statistics: Vec<T>,
    woker_state: Vec<State>,
}

impl<T> WorkerTracker<T>
where
    T: RuntimeStatistic,
{
    pub fn new() -> Self {
        WorkerTracker {
            last_update: vec![],
            peer_ids: vec![],
            batch_sizes: vec![],
            statistics: vec![],
            woker_state: vec![],
        }
    }

    pub fn worker_position(&self, peer_id: &PeerId) -> Result<usize, WorkerTrackerError> {
        match self.peer_ids.iter().position(|n| n == peer_id) {
            Some(idx) => Ok(idx),
            None => Err(WorkerTrackerError::Peer),
        }
    }

    pub fn add_worker(&mut self, id: PeerId, batch_size: u32) {
        self.peer_ids.push(id);
        self.batch_sizes.push(batch_size);
        self.last_update.push(0); // TODO: Need to handle this better for late arrivals
        self.woker_state.push(State::Training);
        self.statistics.push(T::default());
    }

    pub fn remove_worker(&mut self, id: &PeerId) -> Result<(), WorkerTrackerError> {
        let idx = self.worker_position(id)?;
        self.peer_ids.remove(idx);
        self.batch_sizes.remove(idx);
        self.last_update.remove(idx);
        self.woker_state.remove(idx);
        self.statistics.remove(idx);
        Ok(())
    }

    pub fn update(&mut self, id: &PeerId, now: u64) -> Result<(), WorkerTrackerError> {
        let idx = self.worker_position(id)?;
        self.statistics[idx].update(now - self.last_update[idx]);
        self.last_update[idx] = now;
        Ok(())
    }

    pub fn batch_sizes(&self) -> &[u32] {
        self.batch_sizes.as_slice()
    }

    pub fn last_updates(&self) -> &[u64] {
        self.last_update.as_slice()
    }

    pub fn estimates(&self) -> Vec<u64> {
        self.statistics.iter().map(|f| f.value()).collect()
    }

    pub fn worker_state(&self, peer_id: &PeerId) -> State {
        self.woker_state[self.worker_position(peer_id).expect("Position")]
    }

    pub fn update_worker_state(&mut self, peer_id: &PeerId, new_stat: State) {
        let idx = self.worker_position(peer_id).expect("Position");
        self.woker_state[idx] = new_stat;
    }

    pub fn worker(&self) -> Vec<PeerId> {
        self.peer_ids.clone()
    }

    pub fn states(&self) -> Vec<State> {
        self.woker_state.clone()
    }

    pub fn new_round(&mut self) {
        self.last_update = vec![0; self.batch_sizes.len()];
        self.woker_state = vec![State::Training; self.batch_sizes.len()];
    }

    pub fn done(&mut self) {
        self.woker_state = vec![State::Done; self.batch_sizes.len()];
    }
}

impl<T> Default for WorkerTracker<T>
where
    T: RuntimeStatistic,
{
    fn default() -> Self {
        Self::new()
    }
}
