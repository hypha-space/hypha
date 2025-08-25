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

                    let mut file = fs::File::create(&file_name).await.unwrap();
                    let _size = tokio::io::copy(&mut item.reader.compat(), &mut file)
                        .await
                        .unwrap();
                    let _ = fs::set_permissions(&file_name, Permissions::from_mode(0o600)).await;

                    let result_tensor_file_name = match result_tensor {
                        None => {
                            //result_tensor = unsafe { Some(MmapedSafetensors::new(file_name).unwrap()) };
                            file_name
                        }
                        Some((result_tensor_file_name, ref result_tensor)) => {
                            // Average the new tensor with an existing one
                            let new_tensor = unsafe { MmapedSafetensors::new(file_name).unwrap() };

                            fs::create_dir_all(&work_dir.join("avg")).await.unwrap();

                            let mut paths = Vec::new();
                            for (name, tensor) in result_tensor.tensors() {
                                let current_tensor = tensor.load(&device).unwrap();
                                let other_tensor = new_tensor.load(&name, &device).unwrap();

                                let avg_tensor =
                                    ((current_tensor + other_tensor).unwrap() / 2.).unwrap();

                                let avg_tensor_file_name = work_dir.join("avg").join(&name);
                                candle_core::safetensors::save(
                                    &HashMap::from([(name, avg_tensor)]),
                                    avg_tensor_file_name.as_path(),
                                )
                                .unwrap();
                                paths.push(avg_tensor_file_name);
                            }

                            let all_tensors = unsafe { MmapedSafetensors::multi(&paths).unwrap() };
                            serialize_to_file(
                                all_tensors.tensors(),
                                &None,
                                result_tensor_file_name.as_path(),
                            )
                            .unwrap();

                            fs::remove_dir_all(work_dir.join("avg")).await.unwrap();

                            result_tensor_file_name
                        }
                    };

                    result_tensor = Some((result_tensor_file_name.clone(), unsafe {
                        MmapedSafetensors::new(result_tensor_file_name.as_path()).unwrap()
                    }));

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
