use hypha_messages::{Adam, Fetch, Model, ModelType, Nesterov, Preprocessor, PreprocessorType};
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

#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
pub struct PriceRange {
    pub bid: f64,
    pub max: f64,
}

impl Default for PriceRange {
    fn default() -> Self {
        Self {
            bid: 100.0,
            max: 100.0,
        }
    }
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
                model_type: ModelType::ImageClassification,
                input_names: vec!["pixel_values".into(), "labels".into()],
            },
            preprocessor: Some(PreprocessorSource {
                repository: "hypha-space/lenet".to_string(),
                revision: None,
                filenames: vec!["preprocessor_config.json".to_string()],
                token: None,
                preprocessor_type: PreprocessorType::Image,
                input_names: vec!["images".into()],
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
                worker_price: PriceRange::default(),
                parameter_server_price: PriceRange::default(),
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
    pub input_names: Vec<String>,
}

impl From<ModelSource> for Model {
    fn from(source: ModelSource) -> Model {
        Model {
            task: source.model_type,
            artifact: Fetch::huggingface(
                source.repository.clone(),
                source.revision.clone(),
                source.filenames.clone(),
                source.token.clone(),
            ),
            input_names: source.input_names,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct PreprocessorSource {
    pub repository: String,
    pub revision: Option<String>,
    pub filenames: Vec<String>,
    pub token: Option<String>,
    #[serde(rename = "type")]
    pub preprocessor_type: PreprocessorType,
    pub input_names: Vec<String>,
}

impl From<PreprocessorSource> for Preprocessor {
    fn from(source: PreprocessorSource) -> Preprocessor {
        Preprocessor {
            task: source.preprocessor_type,
            artifact: Fetch::huggingface(
                source.repository.clone(),
                source.revision.clone(),
                source.filenames.clone(),
                source.token.clone(),
            ),
            input_names: source.input_names,
        }
    }
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
    #[serde(default)]
    pub worker_price: PriceRange,
    #[serde(default)]
    pub parameter_server_price: PriceRange,
}
