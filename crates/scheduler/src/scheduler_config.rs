use hypha_messages::{
    Adam, DiLoCoConfig, Fetch, HFRepoType, Loss, Nesterov, Requirement, Resources, Scheduler,
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum SchedulerConfig {
    #[serde(rename = "diloco")]
    DiLoCo(DiLoCo),
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        SchedulerConfig::DiLoCo(DiLoCo {
            model: HuggingFaceSource {
                repository: "l45k/Resnet50".to_string(),
                revision: None,
                filenames: vec!["config.json".to_string(), "model.safetensors".to_string()],
                token: None,
            },
            dataset: DataNodeSource {
                dataset: "imagnet".to_string(),
            },
            resources: DiLoCoResources {
                num_workers: 2,
                worker: vec![
                    Requirement::Resource(Resources::Gpu { min: 10.0 }),
                    Requirement::Resource(Resources::Cpu { min: 1.0 }),
                    Requirement::Resource(Resources::Memory { min: 1.0 }),
                    Requirement::Driver {
                        kind: "diloco-transformer".into(),
                    },
                ],
                parameter_server: vec![
                    Requirement::Resource(Resources::Cpu { min: 1.0 }),
                    Requirement::Resource(Resources::Memory { min: 1.0 }),
                    Requirement::Driver {
                        kind: "parameter-server".into(),
                    },
                ],
            },
            worker: WorkerConfig::VisionClassification {
                optimizer: Adam {
                    learning_rate: 1e-3,
                    betas: None,
                    epsilon: None,
                },
                epochs: 3,
                batch_size: 126,
                checkpointing: 1,
                scheduler: None,
                preprocessor: Some(HuggingFaceSource {
                    repository: "l45k/Resnet50".to_string(),
                    revision: None,
                    filenames: vec!["preprocessor_config.json".to_string()],
                    token: None,
                }),
                batches_per_local_epoch: 1,
            },
            parameter_server: ParameterServerConfig {
                optimizer: Nesterov {
                    learning_rate: 0.7,
                    momentum: 0.9,
                },
            },
        })
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DiLoCo {
    pub model: HuggingFaceSource,
    pub dataset: DataNodeSource,
    pub resources: DiLoCoResources,
    pub worker: WorkerConfig,
    pub parameter_server: ParameterServerConfig,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct HuggingFaceSource {
    pub repository: String,
    pub revision: Option<String>,
    pub filenames: Vec<String>,
    pub token: Option<String>,
}

impl From<HuggingFaceSource> for Fetch {
    fn from(source: HuggingFaceSource) -> Self {
        Fetch::huggingface(
            source.repository,
            source.revision,
            source.filenames,
            source.token,
            HFRepoType::Model,
        )
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DataNodeSource {
    pub dataset: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DiLoCoResources {
    pub num_workers: u32,
    pub worker: Vec<Requirement>,

    pub parameter_server: Vec<Requirement>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum WorkerConfig {
    CausalLm {
        optimizer: Adam,
        epochs: i32,
        batch_size: i32,
        checkpointing: i32,
        scheduler: Option<Scheduler>,
    },
    VisionClassification {
        optimizer: Adam,
        epochs: i32,
        batch_size: i32,
        checkpointing: i32,
        scheduler: Option<Scheduler>,
        preprocessor: Option<HuggingFaceSource>,
        batches_per_local_epoch: i32,
    },
    Torch {
        optimizer: Adam,
        loss_fn: Loss,
        epochs: i32,
        batch_size: i32,
        checkpointing: i32,
        scheduler: Option<Scheduler>,
    },
}

impl From<WorkerConfig> for DiLoCoConfig {
    fn from(value: WorkerConfig) -> Self {
        match value {
            WorkerConfig::CausalLm {
                optimizer,
                epochs,
                batch_size,
                checkpointing,
                scheduler,
            } => DiLoCoConfig::CausalLm {
                optimizer,
                epochs,
                batch_size,
                checkpointing,
                scheduler,
            },
            WorkerConfig::VisionClassification {
                optimizer,
                epochs,
                batch_size,
                checkpointing,
                scheduler,
                preprocessor,
                batches_per_local_epoch,
            } => DiLoCoConfig::VisionClassification {
                optimizer,
                epochs,
                batch_size,
                checkpointing,
                scheduler,
                preprocessor: preprocessor.map(|p| p.into()),
                batches_per_local_epoch,
            },
            WorkerConfig::Torch {
                optimizer,
                loss_fn,
                epochs,
                batch_size,
                checkpointing,
                scheduler,
            } => DiLoCoConfig::Torch {
                optimizer,
                loss_fn,
                epochs,
                batch_size,
                checkpointing,
                scheduler,
            },
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ParameterServerConfig {
    pub optimizer: Nesterov,
}
