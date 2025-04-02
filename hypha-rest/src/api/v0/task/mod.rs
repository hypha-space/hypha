use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    pub driver: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatus {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub spec: TaskSpec,
    pub status: TaskStatus,
}

impl Task {
    pub fn new(spec: TaskSpec) -> Self {
        let id = Uuid::new_v4();
        Task {
            id,
            spec,
            status: TaskStatus {
                status: "pending".to_string(),
            },
        }
    }

    pub fn update(&mut self, task: &Task) {
        // Task metadata is immutable, only spec and status can be updated
        self.spec = task.spec.clone();
        self.status = task.status.clone();
    }
}
