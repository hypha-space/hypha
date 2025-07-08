use std::{error::Error, fmt, str::FromStr};

use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Published by schedulers, requesting participating workers to signal
/// their status and available resource to the scheduler.
#[derive(Serialize, Deserialize, Debug)]
pub struct RequestAnnounce {
    pub task_id: Uuid,
    pub scheduler_peer_id: PeerId,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TaskStatus {
    Running,
    Finished,
    Failed,
    Unknown,
}

#[derive(Debug)]
pub struct StatusParseError(String);

impl fmt::Display for StatusParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown task status '{}'", self.0)
    }
}

impl Error for StatusParseError {}

impl FromStr for TaskStatus {
    type Err = StatusParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Running" => Ok(Self::Running),
            "Finished" => Ok(Self::Finished),
            "Failed" => Ok(Self::Failed),
            "Unknown" => Ok(Self::Unknown),
            other => Err(StatusParseError(other.to_string())),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum WorkerRole {
    #[serde(rename = "task")]
    TaskExecutor,
    #[serde(rename = "parameter")]
    ParameterExecutor,
}

/// Requests sent from workers to schedulers.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum WorkerRequest {
    Available { task_id: Uuid, role: WorkerRole },
    TaskStatus { task_id: Uuid, status: TaskStatus },
}

/// Responses to worker requests, sent from schedulers to workers.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum WorkerResponse {
    Available {},
    TaskStatus {},
}

/// Requests sent from schedulers to workers.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SchedulerRequest {
    Work {
        task_id: Uuid,
        parameter_server_peer_id: PeerId,
    },
}

/// Responses to scheduler requests, sent from workers to schedulers.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SchedulerResponse {
    Work {},
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Request {
    Worker(WorkerRequest),
    Scheduler(SchedulerRequest),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Response {
    Worker(WorkerResponse),
    Scheduler(SchedulerResponse),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ArtifactHeader {
    pub task_id: Uuid,
    pub epoch: u64,
}
