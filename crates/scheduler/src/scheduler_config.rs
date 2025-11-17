use hypha_messages::{Adam, Fetch, Model, Nesterov};
use hypha_resources::Resources;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct SchedulerConfig {
    pub job: Job,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            job: Job::Diloco(DiLoCo::default()),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Job {
    Diloco(DiLoCo),
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DiLoCo {
    pub model: ModelSource,
    pub preprocessor: Option<PreprocessorSource>,
    pub dataset: DataNodeSource,
    pub rounds: DiLoCoRounds,
    #[serde(rename = "inner_optimizer")]
    pub inner_optimizer: Adam,
    #[serde(rename = "outer_optimizer")]
    pub outer_optimizer: Nesterov,
    pub resources: DiLoCoResources,
}

impl Default for DiLoCo {
    fn default() -> Self {
        Self {
            model: ModelSource {
                repository: "hypha-space/lenet".to_string(),
                revision: None,
                filenames: vec![
                    "config.json".to_string(),
                    "model.safetensors".to_string(),
                    "configuration_lenet.py".to_string(),
                    "modeling_lenet.py".to_string(),
                ],
                token: None,
                model_type: ModelType::VisionClassification,
            },
            preprocessor: Some(PreprocessorSource {
                repository: "hypha-space/lenet".to_string(),
                revision: None,
                filenames: vec!["preprocessor_config.json".to_string()],
                token: None,
            }),
            dataset: DataNodeSource {
                dataset: "mnist".to_string(),
            },
            rounds: DiLoCoRounds {
                update_rounds: 100,
                avg_samples_between_updates: 1200,
            },
            inner_optimizer: Adam {
                learning_rate: 1e-3,
                betas: None,
                epsilon: None,
            },
            outer_optimizer: Nesterov {
                learning_rate: 0.7,
                momentum: 0.9,
            },
            resources: DiLoCoResources {
                num_workers: 2,
                worker: Resources::default()
                    .with_gpu(10.0)
                    .with_cpu(1.0)
                    .with_memory(1.0),
                parameter_server: Resources::default().with_cpu(1.0).with_memory(1.0),
            },
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ModelSource {
    pub repository: String,
    pub revision: Option<String>,
    pub filenames: Vec<String>,
    pub token: Option<String>,
    #[serde(rename = "type")]
    pub model_type: ModelType,
}

impl From<ModelSource> for Model {
    fn from(source: ModelSource) -> Model {
        let artifact = Fetch::huggingface(
            source.repository.clone(),
            source.revision.clone(),
            source.filenames.clone(),
            source.token.clone(),
        );

        match source.model_type {
            ModelType::VisionClassification => Model::VisionClassification { artifact },
            ModelType::CausalLm => Model::CausalLm { artifact },
            ModelType::Torch => Model::Torch { artifact },
        }
    }
}

impl From<PreprocessorSource> for Fetch {
    fn from(source: PreprocessorSource) -> Fetch {
        Fetch::huggingface(
            source.repository.clone(),
            source.revision.clone(),
            source.filenames.clone(),
            source.token.clone(),
        )
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub enum ModelType {
    VisionClassification,
    CausalLm,
    Torch,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct PreprocessorSource {
    pub repository: String,
    pub revision: Option<String>,
    pub filenames: Vec<String>,
    pub token: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DataNodeSource {
    pub dataset: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DiLoCoRounds {
    pub avg_samples_between_updates: u32,
    pub update_rounds: u32,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DiLoCoResources {
    pub num_workers: u32,
    pub worker: Resources,
    pub parameter_server: Resources,
}
