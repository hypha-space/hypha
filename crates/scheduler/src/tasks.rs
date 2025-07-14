use std::collections::{HashMap, HashSet};

use libp2p::PeerId;
use uuid::Uuid;

pub type TaskId = Uuid;

/// A very simple class to keep track of DiLoCo-like tasks
/// and the participating workers and parameter servers.
pub struct Tasks {
    tasks: HashMap<TaskId, Task>,
}

impl Default for Tasks {
    fn default() -> Self {
        Self::new()
    }
}

impl Tasks {
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    pub fn create_task(&mut self) -> TaskId {
        let task_id = Uuid::new_v4();
        let task = Task {
            workers: HashMap::new(),
            parameter_servers: HashSet::new(),
        };

        self.tasks.insert(task_id, task);

        task_id
    }

    pub fn get_task(&mut self, task_id: &TaskId) -> Option<&mut Task> {
        self.tasks.get_mut(task_id)
    }
}

pub struct Task {
    workers: HashMap<PeerId, hypha_messages::TaskStatus>,
    parameter_servers: HashSet<PeerId>,
}

impl Task {
    pub fn add_worker(&mut self, participant: PeerId) {
        self.workers
            .insert(participant, hypha_messages::TaskStatus::Unknown);
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
        status: hypha_messages::TaskStatus,
    ) -> Option<()> {
        if let Some(current_status) = self.workers.get_mut(participant) {
            *current_status = status;
            Some(())
        } else {
            None
        }
    }
}
