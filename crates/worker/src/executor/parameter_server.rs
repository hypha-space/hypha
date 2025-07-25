use std::{
    collections::HashMap, fs::Permissions, os::unix::fs::PermissionsExt, path::PathBuf, pin::Pin,
};

use candle_core::{
    Device,
    safetensors::{Load, MmapedSafetensors},
};
use futures::StreamExt;
use safetensors::serialize_to_file;
use tokio::fs;
use tokio_util::{
    compat::{FuturesAsyncReadCompatExt, FuturesAsyncWriteCompatExt},
    sync::CancellationToken,
    task::TaskTracker,
};
use uuid::Uuid;

use crate::{
    connector::Connector,
    executor::{Error, Execution, JobExecutor},
    network::Network,
};

pub struct ParameterServerExecutor {
    connector: Connector<Network>,
}

pub struct ParameterServerExecution {
    task_tracker: TaskTracker,
}

impl Execution for ParameterServerExecution {
    fn wait<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.task_tracker.wait().await;
        })
    }
}

impl ParameterServerExecutor {
    pub fn new(connector: Connector<Network>) -> Self {
        ParameterServerExecutor { connector }
    }
}

#[allow(refining_impl_trait)]
impl JobExecutor for ParameterServerExecutor {
    async fn execute(
        &self,
        job: hypha_messages::JobSpec,
        cancel: CancellationToken,
    ) -> Result<ParameterServerExecution, Error> {
        tracing::info!(job_spec = ?job, "Executing parameter server job");

        let id = Uuid::new_v4();
        let work_dir = PathBuf::from(format!("/tmp/hypha-{}", id));
        fs::create_dir_all(&work_dir).await?;

        let device = Device::Cpu;

        let (updates, results) = match job.executor {
            hypha_messages::Executor::ParameterServer { updates, results } => (updates, results),
            _ => return Err(Error::UnsupportedJobSpec()),
        };

        let connector = self.connector.clone();

        let task_tracker = TaskTracker::new();
        task_tracker.spawn(async move {
            let fut = async {
                let mut result_tensor: Option<(PathBuf, MmapedSafetensors)> = None;

                let num_workers = updates.get_peers().len();
                let mut current_worker = 0;

                let mut incoming = match connector.receive(updates).await {
                    Ok(s) => s,
                    Err(_) => return,
                };

                while let Some(item) = incoming.next().await.transpose().ok().flatten() {
                    tracing::info!(file_name = ?item.meta.name, "Received parameter server update");

                    let file_name = work_dir.join(item.meta.name);

                    // NOTE: Be resilient to I/O errors; log and skip faulty updates.
                    let mut file = match fs::File::create(&file_name).await {
                        Ok(f) => f,
                        Err(e) => {
                            tracing::warn!(error = ?e, path = ?file_name, "Failed to create update file");
                            continue;
                        }
                    };
                    if let Err(e) = tokio::io::copy(&mut item.reader.compat(), &mut file).await {
                        tracing::warn!(error = ?e, path = ?file_name, "Failed to write update to file");
                        // Skip this update; the partially written file may be invalid.
                        let _ = fs::remove_file(&file_name).await;
                        continue;
                    }
                    if let Err(e) = fs::set_permissions(&file_name, Permissions::from_mode(0o600)).await {
                        tracing::warn!(error = ?e, path = ?file_name, "Failed to set file permissions");
                    }

                    // NOTE: Prefer continuing with the last good aggregation if averaging fails.
                    // Borrow current aggregation state to avoid moving out of `result_tensor`.
                    let result_tensor_file_name = match result_tensor.as_ref() {
                        None => {
                            file_name
                        }
                        Some((result_tensor_file_name, result_tensor)) => {
                            // Average the new tensor with an existing one
                            match unsafe { MmapedSafetensors::new(file_name) } {
                                Ok(new_tensor) => {
                                    if let Err(e) = fs::create_dir_all(&work_dir.join("avg")).await {
                                        tracing::warn!(error = ?e, "Failed to create avg directory; keeping previous aggregation");
                                        result_tensor_file_name.clone()
                                    } else {
                                        let mut paths = Vec::new();
                                        for (name, tensor) in result_tensor.tensors() {
                                            // Load tensors; skip this tensor on failure
                                            let current_tensor = match tensor.load(&device) {
                                                Ok(t) => t,
                                                Err(e) => {
                                                    tracing::warn!(error = ?e, tensor = %name, "Failed to load current tensor; skipping");
                                                    continue;
                                                }
                                            };
                                            let other_tensor = match new_tensor.load(&name, &device) {
                                                Ok(t) => t,
                                                Err(e) => {
                                                    tracing::warn!(error = ?e, tensor = %name, "Failed to load new tensor; skipping");
                                                    continue;
                                                }
                                            };

                                            let avg_tensor = match (current_tensor + other_tensor).and_then(|t| t / 2.) {
                                                Ok(t) => t,
                                                Err(e) => {
                                                    tracing::warn!(error = ?e, tensor = %name, "Failed to average tensors; skipping");
                                                    continue;
                                                }
                                            };

                                            let avg_tensor_file_name = work_dir.join("avg").join(&name);
                                            if let Err(e) = candle_core::safetensors::save(
                                                &HashMap::from([(name.clone(), avg_tensor)]),
                                                avg_tensor_file_name.as_path(),
                                            ) {
                                                tracing::warn!(error = ?e, tensor = %name, "Failed to save averaged tensor; skipping");
                                                continue;
                                            }
                                            paths.push(avg_tensor_file_name);
                                        }

                                        let mut updated_file = false;
                                        if paths.is_empty() {
                                            tracing::warn!("No tensors averaged; keeping previous aggregation");
                                        } else if let Ok(all_tensors) = unsafe { MmapedSafetensors::multi(&paths) } {
                                            match serialize_to_file(
                                                all_tensors.tensors(),
                                                &None,
                                                result_tensor_file_name.as_path(),
                                            ) {
                                                Ok(_) => {
                                                    updated_file = true;
                                                }
                                                Err(e) => {
                                                    tracing::warn!(error = ?e, "Failed to serialize averaged tensors; keeping previous aggregation");
                                                }
                                            }
                                        } else {
                                            tracing::warn!("Failed to mmap averaged tensors; keeping previous aggregation");
                                        }

                                        if let Err(e) = fs::remove_dir_all(work_dir.join("avg")).await {
                                            tracing::warn!(error = ?e, "Failed to cleanup avg directory");
                                        }

                                        if updated_file {
                                            result_tensor_file_name.clone()
                                        } else {
                                            // Not updated; return previous aggregation file name
                                            result_tensor_file_name.clone()
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(error = ?e, "Failed to mmap new tensor; keeping previous aggregation");

                                    // Keep previous aggregation file
                                    result_tensor_file_name.clone()
                                }
                            }
                        }
                    };

                    // Refresh mmap of the current aggregation; keep previous on failure.
                    match unsafe { MmapedSafetensors::new(result_tensor_file_name.as_path()) } {
                        Ok(mm) => {
                            result_tensor = Some((result_tensor_file_name.clone(), mm));
                        }
                        Err(e) => {
                            tracing::warn!(error = ?e, path = ?result_tensor_file_name, "Failed to mmap aggregation file; keeping previous state");
                        }
                    }

                    current_worker += 1;

                    // We assume that each worker sends their parameters, then waits to receive updates.
                    // With that assumption, we can send these updates after 'num_workers' parameters have been received.
                    // TODO: This needs more work to support more complex scenarios.
                    if current_worker == num_workers {
                        tracing::info!("Sending parameter server update");

                        match connector.send(results.clone()).await {
                            Ok(mut writers) => {
                                while let Some(item_res) = writers.next().await {
                                    match item_res {
                                        Ok(item) => {
                                            if let Some((ref result_file_name, _)) = result_tensor {
                                                match fs::File::open(result_file_name.as_path()).await {
                                                    Ok(mut file) => {
                                                        if let Err(e) = tokio::io::copy(
                                                            &mut file,
                                                            &mut item.writer.compat_write(),
                                                        )
                                                        .await
                                                        {
                                                            tracing::warn!(error = ?e, "Failed to write update to peer");
                                                        }
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(error = ?e, "Failed to open result tensor file");
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!(error = ?e, "Failed to open writer to peer");
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                // Do not panic if peers are not reachable (e.g., no addresses). We'll retry on next batch.
                                tracing::warn!(error = ?e, "Failed to send to peers; will retry on next aggregation");
                            }
                        }

                        current_worker = 0;
                    }
                }
            };

            tokio::select! {
                _ = cancel.cancelled() => {}
                _ = fut => {},
            }
        });

        task_tracker.close();

        Ok(ParameterServerExecution { task_tracker })
    }
}
