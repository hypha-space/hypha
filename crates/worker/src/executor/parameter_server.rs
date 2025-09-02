use std::{
    collections::HashMap, fs::Permissions, os::unix::fs::PermissionsExt, path::PathBuf, pin::Pin,
};

use candle_core::{
    Device,
    safetensors::{Load, MmapedSafetensors},
};
use futures_util::StreamExt;
use safetensors::serialize_to_file;
use tokio::{
    fs,
    io::{self, AsyncWriteExt},
    sync::mpsc,
};
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
    work_dir_base: PathBuf,
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
    pub fn new(connector: Connector<Network>, work_dir_base: PathBuf) -> Self {
        ParameterServerExecutor {
            connector,
            work_dir_base,
        }
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
        let work_dir = self.work_dir_base.join(format!("hypha-{}", id));
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
                let num_workers = updates.get_peers().len();

                // NOTE: Receive streams in parallel, but keep processing (averaging + broadcasting)
                // sequential to stay within memory constraints and preserve existing logic.
                let incoming = match connector.receive(updates).await {
                    Ok(s) => s,
                    Err(_) => return,
                };

                // Channel to report finished file paths from parallel receivers to the sequenced processor.
                let (tx, mut rx) = mpsc::channel::<(String, PathBuf)>(num_workers.max(1) * 2);

                // Spawn a task to accept incoming streams and spawn per-stream copy tasks.
                tokio::spawn( {
                    let work_dir = work_dir.clone();
                    let cancel = cancel.clone();
                    async move {
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            tracing::debug!("stopping parameter accept handler");
                        }
                        _ = incoming.for_each_concurrent(None, |item| {
                            let work_dir = work_dir.clone();
                            let tx = tx.clone();
                            async move {
                                match item {
                                    Ok(item) => {
                                        let name = item.meta.name.clone();
                                        tracing::info!(peer_id = ?name, "Received parameter server update (start)");
                                        let mut reader = item.reader.compat();
                                        let file_name = work_dir.join(name.clone());

                                        match fs::File::create(&file_name).await {
                                            Ok(mut file) => {
                                                match io::copy(&mut reader, &mut file).await {
                                                    Ok(size) => {
                                                        file.sync_all().await.expect("file is synced");
                                                        tracing::info!(size, path = ?file_name, "Wrote update to file");
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(error = ?e, path = ?file_name, "Failed to write update to file");
                                                        let _ = fs::remove_file(&file_name).await;
                                                        return;
                                                    }
                                                }

                                                if let Err(e) = fs::set_permissions(&file_name, Permissions::from_mode(0o600)).await {
                                                    tracing::warn!(error = ?e, path = ?file_name, "Failed to set file permissions");
                                                }
                                                // Notify processor that this update file is ready.
                                                let _ = tx.send((name, file_name)).await;
                                            }
                                            Err(e) => {
                                                tracing::warn!(error = ?e, path = ?file_name, "Failed to create update file");
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = ?e, "Failed to receive parameter server update");
                                    }
                                }
                            }
                        }) => {
                            tracing::warn!("parameter accept handler stopped");
                        }
                    };
                }});

                let mut current_result_tensor_file_name: Option<PathBuf> = None;
                let mut current_worker = 0;

                // Sequentially process completed files as they arrive.
                while let Some((name, file_name)) = rx.recv().await {
                    // NOTE: Prefer continuing with the last good aggregation if averaging fails.
                    // Borrow current aggregation state to avoid moving out of `result_tensor`.

                    let result_tensor_file_name = match current_result_tensor_file_name {
                        None => {
                            // Create a copy of the initial parameters to avoid overwriting mmaped files.
                            // In edge-cases we might receive updates while still sending out results.
                            // If the name of the result file would be the same as one of incoming updates,
                            // we would overwrite an mmapped file.
                            let temporary_tensor_file_name = work_dir.join("avg-temporary");
                            fs::copy(file_name.as_path(), temporary_tensor_file_name.as_path()).await.expect("file can be copied");
                            temporary_tensor_file_name
                        },
                        Some(result_tensor_file_name) => {
                            tokio::task::spawn_blocking({
                                let device = device.clone();
                                let work_dir = work_dir.clone();
                                move || {
                                // Average the new tensor with an existing one
                                match unsafe { MmapedSafetensors::new(file_name) } {
                                    Ok(new_tensor) => {
                                        if let Err(e) = std::fs::create_dir_all(work_dir.join("avg").join(name.as_str())) {
                                            tracing::warn!(error = ?e, "Failed to create avg directory; keeping previous aggregation");
                                            result_tensor_file_name.clone()
                                        } else {
                                            let paths = {
                                                let result_tensor = unsafe { MmapedSafetensors::new(result_tensor_file_name.as_path()) }.expect("tensor file is readable");

                                                let mut paths = Vec::new();
                                                for (tensor_name, tensor) in result_tensor.tensors() {
                                                    // Load tensors; skip this tensor on failure
                                                    let current_tensor = match tensor.load(&device) {
                                                        Ok(t) => t,
                                                        Err(e) => {
                                                            tracing::warn!(error = ?e, tensor = %tensor_name, "Failed to load current tensor; skipping");
                                                            continue;
                                                        }
                                                    };
                                                    let other_tensor = match new_tensor.load(&tensor_name, &device) {
                                                        Ok(t) => t,
                                                        Err(e) => {
                                                            tracing::warn!(error = ?e, tensor = %tensor_name, "Failed to load new tensor; skipping");
                                                            continue;
                                                        }
                                                    };

                                                    let avg_tensor = match (current_tensor + other_tensor).and_then(|t| t / 2.) {
                                                        Ok(t) => t,
                                                        Err(e) => {
                                                            tracing::warn!(error = ?e, tensor = %tensor_name, "Failed to average tensors; skipping");
                                                            continue;
                                                        }
                                                    };

                                                    let avg_tensor_file_name = work_dir.join("avg").join(name.as_str()).join(&tensor_name);
                                                    if let Err(e) = candle_core::safetensors::save(
                                                        &HashMap::from([(tensor_name.clone(), avg_tensor)]),
                                                        avg_tensor_file_name.as_path(),
                                                    ) {
                                                        tracing::warn!(error = ?e, tensor = %tensor_name, "Failed to save averaged tensor; skipping");
                                                        continue;
                                                    }
                                                    paths.push(avg_tensor_file_name);
                                                }

                                                paths
                                            };

                                            if paths.is_empty() {
                                                tracing::warn!("No tensors averaged; keeping previous aggregation");
                                            } else if let Ok(all_tensors) = unsafe { MmapedSafetensors::multi(&paths) } {
                                                // If at this point we're still mmapping 'result_tensor_file_name', we could have undefined behaviour!
                                                // Make sure that we don't!
                                                match serialize_to_file(
                                                    all_tensors.tensors(),
                                                    &None,
                                                    result_tensor_file_name.as_path(),
                                                ) {
                                                    Ok(_) => {
                                                        tracing::debug!("Updated averaged tensors");
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(error = ?e, "Failed to serialize averaged tensors; keeping previous aggregation");
                                                    }
                                                }
                                            } else {
                                                tracing::warn!("Failed to mmap averaged tensors; keeping previous aggregation");
                                            }

                                            if let Err(e) = std::fs::remove_dir_all(work_dir.join("avg").join(name.as_str())) {
                                                tracing::warn!(error = ?e, "Failed to cleanup avg directory");
                                            }

                                            result_tensor_file_name.clone()
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = ?e, "Failed to mmap new tensor; keeping previous aggregation");

                                        // Keep previous aggregation file
                                        result_tensor_file_name.clone()
                                    }
                                }
                            }}).await.expect("averaging tasks runs to completion")
                        }
                    };

                    current_result_tensor_file_name = Some(result_tensor_file_name);
                    current_worker += 1;

                    // We assume that each worker sends their parameters, then waits to receive updates.
                    // With that assumption, we can send these updates after 'num_workers' parameters have been received.
                    // TODO: This needs more work to support more complex scenarios.
                    if current_worker == num_workers && let Some(ref result_file_name) = current_result_tensor_file_name {
                        // Rename the temporary result to ensure that it's available for updates again.
                        // In edge-cases we might receive updates while still sending out results.
                        // This will overwrite 'result_file_name'.
                        let final_tensor_file_name = work_dir.join("avg-final");
                        fs::rename(result_file_name.as_path(), final_tensor_file_name.as_path()).await.expect("tensor file can be renamed");

                        // These need to be reset before sending out the result!
                        current_result_tensor_file_name = None;
                        current_worker = 0;

                        match connector.send(results.clone()).await {
                            Ok(writers) => {
                                writers.for_each_concurrent(None, async |item_res| {
                                    match item_res {
                                        Ok(item) => {
                                                tracing::info!(peer_id = item.meta.name, "Sending parameter server update");

                                                match fs::File::open(final_tensor_file_name.as_path()).await {
                                                    Ok(mut file) => {
                                                        let mut writer = item.writer.compat_write();

                                                        if let Err(e) = io::copy(
                                                            &mut file,
                                                            &mut writer,
                                                        )
                                                        .await
                                                        {
                                                            tracing::warn!(error = ?e, "Failed to write update to peer");
                                                        }

                                                        writer.shutdown().await.expect("Failed to shutdown writer");
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(error = ?e, "Failed to open result tensor file");
                                                    }
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!(error = ?e, "Failed to open writer to peer");
                                        }
                                    }
                                }).await;
                            }
                            Err(e) => {
                                // Do not panic if peers are not reachable (e.g., no addresses). We'll retry on next batch.
                                tracing::warn!(error = ?e, "Failed to send to peers; will retry on next aggregation");
                            }
                        }

                        fs::remove_file(final_tensor_file_name.as_path()).await.expect("tensor file can be removed");
                    }
                }
            };

            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::debug!("stopping parameter server");
                }
                _ = fut => {
                    tracing::warn!("parameter server stopped");
                },
            }

            // Clean up.
            let _ = fs::remove_dir_all(&work_dir).await;
        });

        task_tracker.close();

        Ok(ParameterServerExecution { task_tracker })
    }
}
