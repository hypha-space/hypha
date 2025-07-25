use std::collections::{HashMap, HashSet};

use libp2p::PeerId;
use uuid::Uuid;

pub type JobId = Uuid;

/// A very simple class to keep track of DiLoCo-like jobs
/// and the participating workers and parameter servers.
pub struct Jobs {
    jobs: HashMap<JobId, Job>,
}

impl Default for Jobs {
    fn default() -> Self {
        Self::new()
    }
}

impl Jobs {
    pub fn new() -> Self {
        Self {
            jobs: HashMap::new(),
        }
    }

    pub fn create_job(&mut self) -> JobId {
        let job_id = Uuid::new_v4();
        let job = Job {
            workers: HashMap::new(),
            parameter_servers: HashSet::new(),
        };

        self.jobs.insert(job_id, job);

        job_id
    }

    pub fn get_job(&mut self, job_id: &JobId) -> Option<&mut Job> {
        self.jobs.get_mut(job_id)
    }
}

pub struct Job {
    workers: HashMap<PeerId, hypha_messages::JobStatus>,
    parameter_servers: HashSet<PeerId>,
}

impl Job {
    pub fn add_worker(&mut self, participant: PeerId) {
        self.workers
            .insert(participant, hypha_messages::JobStatus::Unknown);
    }

    pub fn add_parameter_server(&mut self, participant: PeerId) {
        self.parameter_servers.insert(participant);
    }

    pub fn workers(&self) -> impl Iterator<Item = &PeerId> {
        self.workers.keys()
    }

    // HACK! right now we assume a single parameter server.
    // We need to support multiple though.
    pub fn parameter_server(&self) -> Option<PeerId> {
        self.parameter_servers.iter().next().cloned()
    }

    pub fn update_status(
        &mut self,
        participant: &PeerId,
        status: hypha_messages::JobStatus,
    ) -> Option<()> {
        if let Some(current_status) = self.workers.get_mut(participant) {
            *current_status = status;
            Some(())
        } else {
            None
        }
    }
}
