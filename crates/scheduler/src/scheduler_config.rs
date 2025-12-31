use documented::{Documented, DocumentedFieldsOpt};
use hypha_messages::{Adam, Fetch, Model, ModelType, Nesterov, Preprocessor, PreprocessorType};
use hypha_resources::Resources;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
#[serde(tag = "type", rename_all = "lowercase")]
/// Metrics configuration for forwarding training metrics.
///
/// Selects where training metrics are forwarded:
/// * `aim` - Send metrics to an AIM relay endpoint (e.g., "0.0.0.0:61000")
/// * `otel` - Record metrics via the configured OTEL exporter using gauges.
/// * `csv` - Append metrics as CSV rows (Trackio compatible).
/// * `jsonl` - Append metrics as JSON lines (one per record).
///
/// You can provide a single entry or an array to fan out metrics to multiple sinks.
pub enum MetricsConfig {
    /// Send metrics to an AIM relay endpoint.
    Aim { endpoint: String },
    /// Record metrics via the configured OTEL exporter using gauges.
    Otel,
    /// Append metrics as CSV rows to a local file (Trackio-compatible header/columns).
    Csv { path: String },
    /// Append metrics as JSON lines to a local file.
    Jsonl { path: String },
}

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
    pub metrics: Vec<MetricsConfig>,
    #[serde(rename = "inner_optimizer")]
    pub inner_optimizer: Adam,
    #[serde(rename = "outer_optimizer")]
    pub outer_optimizer: Nesterov,
    pub resources: DiLoCoResources,
    pub model_destination: Option<ModelDestination>,
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
                filenames: vec![
                    "preprocessor_config.json".to_string(),
                    "preprocessor_lenet.py".to_string(),
                ],
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
                max_batch_size: Some(600),
                multi_batch_size: 3,
            },
            metrics: vec![
                MetricsConfig::Otel,
                MetricsConfig::Jsonl {
                    path: "metrics.jsonl".to_string(),
                },
                MetricsConfig::Csv {
                    path: "metrics.csv".to_string(),
                },
            ],
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
                worker: Resources::default()
                    .with_gpu(0.11)
                    .with_cpu(1.0)
                    .with_memory(1.0),
                parameter_server: Resources::default().with_cpu(1.0).with_memory(1.0),
                worker_price: PriceRange::default(),
                parameter_server_price: PriceRange::default(),
                worker_pool: PoolSettings {
                    min: 2,
                    target: 2,
                    grace_ms: PoolSettings::default_grace_ms(),
                },
                parameter_server_pool: PoolSettings {
                    min: 1,
                    target: 1,
                    grace_ms: PoolSettings::default_grace_ms(),
                },
            },
            model_destination: None,
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
pub struct ModelDestination {
    pub repository: String,
    pub token: String,
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
    pub max_batch_size: Option<u32>,
    pub multi_batch_size: u32,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
pub struct PoolSettings {
    /// Minimum number of members required.
    pub min: u32,
    /// Target number of members to pursue.
    pub target: u32,
    /// Grace period (milliseconds) before failing when below min.
    #[serde(default = "PoolSettings::default_grace_ms")]
    pub grace_ms: u64,
}

impl PoolSettings {
    const fn default_grace_ms() -> u64 {
        10_000
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct DiLoCoResources {
    pub worker: Resources,
    pub parameter_server: Resources,
    #[serde(default)]
    pub worker_price: PriceRange,
    #[serde(default)]
    pub parameter_server_price: PriceRange,
    pub worker_pool: PoolSettings,
    pub parameter_server_pool: PoolSettings,
}
