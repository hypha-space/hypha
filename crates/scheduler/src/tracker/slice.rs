use std::{collections::HashMap, ops::BitAnd};

use itertools::Itertools;
use libp2p::PeerId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SliceError {
    #[error("Not found")]
    NotFound,
    #[error("Slice is in a wrong state")]
    State,
    #[error("Error during scheduling {0}")]
    Scheduling(String),
}

#[derive(Debug, Clone, Copy)]
pub struct Slice {
    id: u64,
    peer: Option<PeerId>,
    processed: bool,
}

impl Slice {
    pub fn new(id: u64) -> Self {
        Slice {
            id,
            peer: None,
            processed: false,
        }
    }
}

/// This tracks the data slices provided by a data node.
pub struct SliceTracker {
    slices: Vec<Slice>,
    processing: HashMap<PeerId, u64>,
    rounds: u32,
}

impl SliceTracker {
    pub fn new(num_slices: u64) -> Self {
        Self {
            slices: Vec::from_iter((0..num_slices).map(Slice::new)),
            processing: HashMap::new(),
            rounds: 0,
        }
    }

    pub fn next(&mut self, peer: &PeerId) -> u64 {
        let open_slice = self
            .slices
            .iter_mut()
            .find(|s| (!s.processed).bitand(s.peer.is_none_or(|id| &id == peer)));

        // check if something is open
        match open_slice {
            Some(slice) => {
                slice.processed = true;
                slice.peer = Some(*peer);
                self.processing.insert(*peer, slice.id);
                slice.id
            }
            None => {
                // We use a cache stealing strategy. If a worker is slower, other workers can steal their cached data.
                let open_slices = self.slices.iter().filter(|&s| !s.processed);
                let mut counts: HashMap<PeerId, u32> = HashMap::new();
                for s in open_slices {
                    // unwrap is fine otherwise the open_slice filter would have failed
                    let slice_peer_id = s.peer.expect("Slice");
                    counts
                        .entry(slice_peer_id)
                        .and_modify(|f| *f += 1)
                        .or_insert(0);
                }
                let slow_peer = counts.iter().sorted_by_key(|f| f.1).next();
                match slow_peer {
                    Some((p, count)) => {
                        tracing::info!(peer=%p, open_slices=&count, "Slowest peer with open slices. Steal slice.");
                        // unwrap is ok since we have at least one entry
                        let slice = self
                            .slices
                            .iter_mut()
                            .find(|s| (!s.processed).bitand(s.peer.is_none_or(|id| &id == p)))
                            .expect("Slice");
                        slice.processed = true;
                        slice.peer = Some(*peer);
                        self.processing.insert(*peer, slice.id);
                        slice.id
                    }
                    None => {
                        tracing::info!("All slices have been processed. Start new round");
                        self.rounds += 1;
                        for s in self.slices.iter_mut() {
                            s.processed = false;
                        }
                        // Start again
                        self.next(peer)
                    }
                }
            }
        }
    }

    pub fn remove_worker(&mut self, peer: &PeerId) {
        while let Some(s) = self.slices.iter_mut().find(|s| s.peer == Some(*peer)) {
            s.peer = None;
        }
        if let Some(s_id) = self.processing.remove(peer)
            && let Some(slice) = self.slices.iter_mut().find(|s| s.id == s_id)
        {
            slice.processed = false;
        }
    }
}

#[cfg(test)]
mod batch_scheduler_tests {
    use libp2p::PeerId;

    use super::SliceTracker;

    #[tokio::test]
    async fn simple_scheduling() {
        let peer_1 = PeerId::random();
        let peer_2 = PeerId::random();
        let num_slices = 7;

        let mut tracker = SliceTracker::new(num_slices);

        assert_eq!(tracker.next(&peer_1), 0);
        assert_eq!(tracker.next(&peer_2), 1);
        assert_eq!(tracker.next(&peer_1), 2);
        assert_eq!(tracker.next(&peer_2), 3);
        assert_eq!(tracker.next(&peer_1), 4);
        assert_eq!(tracker.next(&peer_2), 5);
        assert_eq!(tracker.next(&peer_1), 6);
        assert_eq!(tracker.next(&peer_2), 1);
        assert_eq!(tracker.next(&peer_1), 0);
    }

    #[tokio::test]
    async fn fast_slow_scheduling() {
        let peer_1 = PeerId::random();
        let peer_2 = PeerId::random();
        let num_slices = 7;

        let mut tracker = SliceTracker::new(num_slices);

        // Round 0
        assert_eq!(tracker.next(&peer_1), 0);
        assert_eq!(tracker.next(&peer_1), 1);
        assert_eq!(tracker.next(&peer_1), 2);
        assert_eq!(tracker.next(&peer_2), 3);
        assert_eq!(tracker.next(&peer_1), 4);
        assert_eq!(tracker.next(&peer_1), 5);
        assert_eq!(tracker.next(&peer_1), 6);
        // Round 1
        assert_eq!(tracker.next(&peer_2), 3);
        assert_eq!(tracker.next(&peer_1), 0);
        assert_eq!(tracker.next(&peer_1), 1);
        assert_eq!(tracker.next(&peer_1), 2);
        let slice = tracker.next(&peer_2);
        let mut left_slices = vec![4, 5, 6];
        assert!(left_slices.contains(&slice));
        left_slices.retain(|f| f != &slice);
        assert!(left_slices.contains(&tracker.next(&peer_1)));
        assert!(left_slices.contains(&tracker.next(&peer_1)));
        // Round 2
        assert_eq!(tracker.next(&peer_1), 0);
        assert_eq!(tracker.next(&peer_2), 3);
        assert_eq!(tracker.rounds, 2);
    }

    #[tokio::test]
    async fn peer_removed_scheduling() {
        let peer_1 = PeerId::random();
        let peer_2 = PeerId::random();
        let peer_3 = PeerId::random();
        let num_slices = 7;

        let mut tracker = SliceTracker::new(num_slices);

        // Round 0
        assert_eq!(tracker.next(&peer_1), 0);
        assert_eq!(tracker.next(&peer_2), 1);
        assert_eq!(tracker.next(&peer_1), 2);
        assert_eq!(tracker.next(&peer_2), 3);
        assert_eq!(tracker.next(&peer_1), 4);
        assert_eq!(tracker.next(&peer_2), 5);
        assert_eq!(tracker.next(&peer_1), 6);
        tracker.remove_worker(&peer_1);
        assert_eq!(tracker.next(&peer_3), 6);
        // Round 1
        assert_eq!(tracker.next(&peer_2), 0);
        assert_eq!(tracker.next(&peer_3), 2);
        assert_eq!(tracker.next(&peer_2), 1);
        assert_eq!(tracker.next(&peer_3), 4);
        assert_eq!(tracker.next(&peer_2), 3);
        assert_eq!(tracker.next(&peer_3), 6);
        assert_eq!(tracker.next(&peer_2), 5);
    }
}
