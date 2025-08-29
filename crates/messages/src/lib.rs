use std::{error::Error, fmt, str::FromStr, time::SystemTime};

use libp2p::PeerId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// TODO: Move into a separate module
#[derive(Serialize, Deserialize, Debug)]
pub struct ArtifactHeader {
    pub job_id: Uuid,
    pub epoch: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    WorkerOffer(worker_offer::Request),
    RenewLease(renew_lease::Request),
    JobStatus(job_status::Request),
    DispatchJob(dispatch_job::Request),
    ParameterPull(parameter_pull::Request),
    ParameterPush(parameter_push::Request),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    WorkerOffer(worker_offer::Response),
    RenewLease(renew_lease::Response),
    JobStatus(job_status::Response),
    DispatchJob(dispatch_job::Response),
    ParameterPull(parameter_pull::Response),
    ParameterPush(parameter_push::Response),
}

// Protocol: Scheduler requests available workers
pub mod request_worker {

    use super::*;

    /// Scheduler broadcasts to find available workers matching requirements
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Request {
        pub id: Uuid,
        pub spec: WorkerSpec,
        pub timeout: SystemTime,
        pub bid: f64,
    }
}

// Protocol: Worker proactively offers availability
pub mod worker_offer {

    use super::*;

    /// Worker offers a lease to scheduler
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Request {
        pub id: Uuid,
        pub request_id: Uuid,
        /// Worker's _counter-offer_ price
        pub price: f64,
        /// Accept the offer within the timeout otherwise it's going to expire.
        pub timeout: SystemTime,
    }

    /// Scheduler acknowledges the offer
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Response {}
}

// Protocol: Renew an active lease
pub mod renew_lease {
    use super::*;

    /// Scheduler requests to extend the lease
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Request {
        pub id: Uuid,
    }

    /// Worker acknowledges the extension
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub enum Response {
        Renewed {
            id: Uuid,
            /// New timeout for the lease
            timeout: SystemTime,
        },
        Failed,
    }
}

pub mod dispatch_job {
    use super::*;

    /// Scheduler requests to dispatch a job to a worker
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Request {
        pub id: Uuid,
        pub spec: JobSpec,
    }

    /// Worker acknowledges the dispatch
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub enum Response {
        Dispatched {
            id: Uuid,
            /// New timeout for the lease
            timeout: SystemTime,
        },
        Failed,
    }
}

pub mod job_status {
    use super::*;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Request {
        pub job_id: Uuid,
        pub status: JobStatus,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Response {}
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobSpec {
    /// Executor configuration
    pub executor: Executor,
}

/// Worker specification
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkerSpec {
    /// List of requirements for job execution (ALL must match)
    pub requirements: Vec<Requirement>,
}

/// A single requirement that a worker must fulfill to execute a job
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum Requirement {
    /// Hardware requirement
    Resource(Resources),
    // We're currently ignoring driver requirements
    // Driver, Service
    Driver {
        kind: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum SelectionStrategy {
    All,
    Random,
    One,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum HFRepoType {
    Model,
    Dataset,
}

/// Reference types for pointing to models, data, or other resources
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum Reference {
    // TODO: Add support for bitswap like loading via CIDs
    // Content Identifieres (content-addressed)
    // #[serde(rename = "cids")]
    // Cids { values: Vec<String> },
    /// URI reference for fetching static data (models, datasets)
    /// Intent: Pull-based, request/response
    #[serde(rename = "uri")]
    Uri { value: String },

    /// Hugging Face model reference for fetching models
    /// Intent: Pull-based, request/response
    /// NOTE: Doesn't support auth!
    #[serde(rename = "huggingface")]
    HuggingFace {
        repository: String,
        revision: Option<String>,
        filenames: Vec<String>,
        token: Option<String>,
        repo_type: HFRepoType,
    },

    #[serde(rename = "peers")]
    Peers {
        peers: Vec<PeerId>,
        strategy: SelectionStrategy,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "Reference", into = "Reference")]
pub struct Fetch(Reference);

impl Fetch {
    pub fn uri(value: impl Into<String>) -> Self {
        Self(Reference::Uri {
            value: value.into(),
        })
    }

    pub fn huggingface(
        repository: impl Into<String>,
        revision: Option<String>,
        filenames: Vec<String>,
        token: Option<String>,
        repo_type: HFRepoType,
    ) -> Self {
        Self(Reference::HuggingFace {
            repository: repository.into(),
            revision,
            filenames,
            token,
            repo_type,
        })
    }

    pub fn as_ref(&self) -> &Reference {
        &self.0
    }
}

impl TryFrom<Reference> for Fetch {
    type Error = &'static str;
    fn try_from(r: Reference) -> Result<Self, Self::Error> {
        match r {
            Reference::Uri { .. } | Reference::HuggingFace { .. } => Ok(Fetch(r)),
            _ => Err("Fetch can only be created from Uri or HuggingFace"),
        }
    }
}

impl From<Fetch> for Reference {
    fn from(f: Fetch) -> Reference {
        f.0
    }
}

impl AsRef<Reference> for Fetch {
    fn as_ref(&self) -> &Reference {
        &self.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "Reference", into = "Reference")]
pub struct Send(Reference);

impl Send {
    pub fn peers(peers: Vec<PeerId>, strategy: SelectionStrategy) -> Self {
        Self(Reference::Peers { peers, strategy })
    }

    pub fn as_ref(&self) -> &Reference {
        &self.0
    }
}

impl TryFrom<Reference> for Send {
    type Error = &'static str;
    fn try_from(r: Reference) -> Result<Self, Self::Error> {
        match r {
            Reference::Peers { .. } => Ok(Send(r)),
            _ => Err("Send can only be created from Peers"),
        }
    }
}

impl From<Send> for Reference {
    fn from(s: Send) -> Reference {
        s.0
    }
}

impl AsRef<Reference> for Send {
    fn as_ref(&self) -> &Reference {
        &self.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "Reference", into = "Reference")]
pub struct Receive(Reference);

impl Receive {
    pub fn peers(peers: Vec<PeerId>) -> Self {
        Self(Reference::Peers {
            peers,
            strategy: SelectionStrategy::All,
        })
    }

    pub fn as_ref(&self) -> &Reference {
        &self.0
    }

    pub fn get_peers(&self) -> &Vec<PeerId> {
        match &self.0 {
            Reference::Peers { peers, .. } => peers,
            _ => panic!(),
        }
    }
}

impl TryFrom<Reference> for Receive {
    type Error = &'static str;

    fn try_from(r: Reference) -> Result<Self, Self::Error> {
        match r {
            Reference::Peers {
                strategy: SelectionStrategy::All,
                ..
            } => Ok(Receive(r)),
            Reference::Peers { .. } => Err("Receive requires SelectionStrategy::All"),
            _ => Err("Receive can only be created from Peers"),
        }
    }
}

impl From<Receive> for Reference {
    fn from(r: Receive) -> Reference {
        r.0
    }
}

impl AsRef<Reference> for Receive {
    fn as_ref(&self) -> &Reference {
        &self.0
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum DiLoCoConfig {
    CausalLm {
        optimizer: Optimizer,
        epochs: i32,
        batch_size: i32,
        checkpointing: i32,
        scheduler: Option<Scheduler>,
    },
    Torch {
        optimizer: Optimizer,
        loss_fn: Loss,
        epochs: i32,
        batch_size: i32,
        checkpointing: i32,
        scheduler: Option<Scheduler>,
    },
}

// impl TryFrom<Mode> for AllowedMode {
//     type Error = ();
//     fn try_from(m: Mode) -> Result<Self, ()> {
//         match m {
//             Mode::A | Mode::B => Ok(Self(m)),
//             _ => Err(()),
//         }
//     }
// }

// impl From<AllowedMode> for Mode {
//     fn from(am: AllowedMode) -> Mode {
//         am.0
//     }
// }

/// Driver specifications for different ML frameworks
#[derive(Serialize, Deserialize, Debug, Clone)]
#[non_exhaustive]
#[serde(tag = "type")]
pub enum Executor {
    #[serde(rename = "diloco-transformer")]
    DiLoCoTransformer {
        model: Fetch,
        data: Fetch,
        updates: Receive,
        results: Send,
        config: DiLoCoConfig,
    },
    #[serde(rename = "parameter-server")]
    ParameterServer { updates: Receive, results: Send },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum Optimizer {
    Adam {
        learning_rate: f64,
        betas: Option<[f64; 2]>,
        epsilon: Option<f64>,
    },
    SGD {
        learning_rate: f64,
        momentum: Option<f64>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum Loss {
    L1,
    MSE,
    CrossEntropyLoss,
    BCEWithLogits,
    KLDivLoss,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum Scheduler {
    CosineWithWarmup {
        warmup_steps: i32,
        training_steps: i32,
    },
    LinearWithWarmup {
        warmup_steps: i32,
        training_steps: i32,
    },
    WSD {
        warmup_steps: i32,
        decay_steps: i32,
    },
}

/// Hardware resource requirements
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum Resources {
    #[serde(rename = "GPU")]
    Gpu {
        min: f64,
    },
    #[serde(rename = "CPU")]
    Cpu {
        min: f64,
    },
    Storage {
        min: f64,
    },
    Memory {
        min: f64,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum JobStatus {
    Running,
    Finished,
    Failed,
    Unknown,
}

impl FromStr for JobStatus {
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

#[derive(Debug)]
pub struct StatusParseError(String);

impl fmt::Display for StatusParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown job status '{}'", self.0)
    }
}

impl Error for StatusParseError {}

// Protocol: Pull parameters from parameter server
pub mod parameter_pull {
    use super::*;

    /// Worker requests to pull parameters from parameter server
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Request {
        pub job_id: Uuid,
        pub key: String,
        pub version: Option<u64>,
    }

    /// Parameter server responds with parameters or error
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub enum Response {
        Success { version: u64, data_stream_id: Uuid },
        NotFound,
        Error(String),
    }
}

// Protocol: Push parameters to parameter server
pub mod parameter_push {
    use super::*;

    /// Worker pushes parameters to parameter server
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Request {
        pub job_id: Uuid,
        pub key: String,
        pub version: Option<u64>,
        pub data_stream_id: Uuid,
        pub data_size: u64,
    }

    /// Parameter server acknowledges parameter storage
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub enum Response {
        Success { version: u64 },
        Error(String),
    }
}

/// Header for parameter data streams
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterStreamHeader {
    pub stream_id: Uuid,
    pub data_size: u64,
}
