use std::{
    collections::HashMap,
    fs::Permissions,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    pin::Pin,
};

use candle_core::{
    Device, Tensor,
    safetensors::{Load, MmapedSafetensors},
};
use futures_util::StreamExt;
use safetensors::serialize_to_file;
use sha2::{Digest, Sha256};
use tokio::{
    fs,
    io::{self, AsyncWriteExt},
    sync::mpsc,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

use crate::{
    connector::Connector,
    executor::{Error, Execution, JobExecutor},
    network::Network,
};

#[derive(Debug, thiserror::Error)]
pub enum TensorOpError {
    #[error("Candle core error: {0}")]
    Candle(#[from] candle_core::Error),

    #[error("Safetensors error: {0}")]
    Safetensor(#[from] safetensors::SafeTensorError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

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

        let (updates, results, optimizer) = match job.executor {
            hypha_messages::Executor::ParameterServer {
                updates,
                results,
                optimizer,
            } => (updates, results, optimizer),
            _ => return Err(Error::UnsupportedJobSpec()),
        };

        let (learning_rate, momentum) = match optimizer {
            hypha_messages::Optimizer::Nesterov {
                learning_rate,
                momentum,
            } => (learning_rate, momentum),
            _ => return Err(Error::UnsupportedOptimizer()),
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
                                        let mut reader = item.reader;
                                        // Don't use the name from the meta data as it could be an arbitrary path.
                                        let hex_digest = format!("{:X}",Sha256::digest(item.meta.name));
                                        tracing::info!(peer_id = ?name, file_name = ?hex_digest, "Received parameter server update (start)");
                                        let file_name = work_dir.join(hex_digest);

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

                    tracing::info!("Received file {:?}", name);
                    current_result_tensor_file_name = match current_result_tensor_file_name {
                        None => {
                            // Just use the first file as the current name, so no copy required.
                            Some(file_name.to_path_buf())
                        },
                        Some(result_tensor_file_name) => {
                                let work_dir = work_dir.clone();
                                let resulting_tensor_file_name = work_dir.join(format!("joined_{:?}",  Uuid::new_v4()));

                                // TODO: This isn't correct. We need a weighting with the number of samples processed by
                                // the worker. Until we have this information lets assume we traing with two workers.
                                // Average the new tensor with an existing one
                                let average_op = |a: &Tensor, b: &Tensor|{
                                    // Compute (a + b) / 2.
                                    (a + b).and_then(|t| t / 2.)
                                };
                                if let Err(e) = apply_tensor_op(
                                    &file_name,
                                    &result_tensor_file_name,
                                    &resulting_tensor_file_name,
                                    &work_dir,
                                    &device,
                                    average_op,
                                )
                                .await{
                                    tracing::warn!(error = ?e, "Failed to average results");
                                }
                                Some(resulting_tensor_file_name)
                        }
                    };

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

                        // Do outer optimization
                        let gradient_file =  nesterov(final_tensor_file_name.clone(),  work_dir.clone(), &device, momentum, learning_rate).await.expect("nesterov");

                        // These need to be reset before sending out the result!
                        current_result_tensor_file_name = None;
                        current_worker = 0;

                        match connector.send(results.clone()).await {
                            Ok(writers) => {
                                writers.for_each_concurrent(None, async |item_res| {
                                    match item_res {
                                        Ok(item) => {
                                                tracing::info!(peer_id = item.meta.name, "Sending parameter server update");

                                                match fs::File::open(gradient_file.as_path()).await {
                                                    Ok(mut file) => {
                                                        let mut writer = item.writer;

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

                        fs::remove_file(gradient_file.as_path()).await.expect("gradient file can be removed");
                        fs::remove_file(final_tensor_file_name.as_path()).await.expect("tensor file can be removed")
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

/// Applies a binary operation to corresponding tensors from two safetensor files.
///
/// This function memory-maps two input safetensor files, iterates through the tensors
/// of the first file, finds the corresponding tensor by name in the second file,
/// and applies the provided operation `op` to the pair. The resulting tensors
/// are saved into a `temp_path` and combined into a single result file. Only
/// two tensors will be held in memory at the same time.
///
/// # Arguments
/// * `file_a_path` - Path to the first safetensor file.
/// * `file_b_path` - Path to the second safetensor file.
/// * `output_path` - Path where the resulting safetensor file will be saved.
/// * `temp_path` - Path to a temporary directory where the intermediate safetensor file will be saved.
/// * `device` - The Candle device to perform computations on (e.g., `Device::Cpu`).
/// * `op` - A closure that takes two tensors and returns a new tensor.
///
/// # Returns
/// A `Result` indicating success or a `TensorOpError` on failure.
///
/// # Note
/// Tensors present in the second file but not the first are ignored. If a tensor
/// from the first file is not found in the second, it is skipped with a warning.
/// The `temp_path` will be created and delted by the function. Make sure it doesn't
/// point to an existing directory that contains important data.
/// Also make sure that the tensors in `op` are in the same order as they are passed to the function.
async fn apply_tensor_op<F>(
    file_a_path: &Path,
    file_b_path: &Path,
    output_path: &Path,
    temp_path: &Path,
    device: &Device,
    op: F,
) -> Result<(), TensorOpError>
where
    F: Fn(&Tensor, &Tensor) -> Result<Tensor, candle_core::Error>,
{
    // 1. Open both safetensor files in a memory-mapped way.
    // SAFETY: The MmapedSafetensors::new function is unsafe because it assumes
    // the underlying file will not be modified while the memory map is active.
    let tensors_a = unsafe { candle_core::safetensors::MmapedSafetensors::new(file_a_path)? };
    let tensors_b = unsafe { candle_core::safetensors::MmapedSafetensors::new(file_b_path)? };

    let mut result_tensors = Vec::new();

    // 2. Iterate through each tensor in the first file.
    for (name, tensor_view) in tensors_a.tensors() {
        // Try to load the corresponding tensor from the second file.
        match tensors_b.load(&name, device) {
            Ok(tensor_b) => {
                // If found, load the tensor from the first file.
                let tensor_a = tensor_view.load(device)?;

                // 3. Apply the provided computation function and serialize the result to disk.
                let result_tensor = op(&tensor_a, &tensor_b)?;
                let result_path = temp_path.join(format!("{:X}", Sha256::digest(name.clone())));
                candle_core::safetensors::save(
                    &HashMap::from([(name, result_tensor)]),
                    result_path.clone(),
                )?;
                result_tensors.push(result_path);
            }
            Err(_) => {
                // If a tensor from file A doesn't exist in file B, skip it.
                tracing::warn!("Tensor '{}' not found in second file, skipping.", name);
                continue;
            }
        }
    }

    // 4. Write all result tensors to the new file.
    if result_tensors.is_empty() {
        tracing::warn!("Warning: No matching tensors found to process.");
    } else {
        let all_tensors = unsafe { MmapedSafetensors::multi(&result_tensors)? };
        serialize_to_file(all_tensors.tensors(), &None, output_path)?;
    }

    Ok(())
}

async fn update_momentum(
    work_dir: PathBuf,
    gradient_file_name: &Path,
    device: &Device,
    momentum: f64,
) -> Result<PathBuf, Error> {
    // If we are in the first round, we need to initialize the momentum with the gradient
    let momentum_file = work_dir.join("momentum");
    if fs::metadata(momentum_file.clone()).await.is_err() {
        fs::copy(gradient_file_name.to_path_buf(), momentum_file.clone())
            .await
            .expect("copy gradients to momentum");
    } else {
        let momentum_update_file = work_dir.join("momentum_update");
        let momentum_op = |g: &Tensor, m: &Tensor| {
            // Calculation: (mu * momentum) / 2.0
            (momentum * m).and_then(|t| t + g)
        };
        apply_tensor_op(
            gradient_file_name,
            &momentum_file,
            &momentum_update_file,
            &work_dir,
            device,
            momentum_op,
        )
        .await?;
        fs::copy(momentum_update_file, momentum_file.clone()).await?;
    }
    Ok(momentum_file)
}

async fn nesterov(
    gradient_file: PathBuf,
    work_dir: PathBuf,
    device: &Device,
    momentum: f64,
    learning_rate: f64,
) -> Result<PathBuf, Error> {
    let momentum_file = update_momentum(work_dir.clone(), &gradient_file, device, momentum).await?;

    let result_gradient_name = work_dir.join("gradient_update");

    let nesterov_op = |g: &Tensor, m: &Tensor| {
        // Compute: learning_rate * ((momentum * m) + g)
        (momentum * m)
            .and_then(|t| t + g)
            .and_then(|t| learning_rate * t)
    };
    apply_tensor_op(
        &gradient_file,
        &momentum_file,
        &result_gradient_name,
        &work_dir,
        device,
        nesterov_op,
    )
    .await?;

    Ok(result_gradient_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Belows test is based on the following python code
    // import torch
    // param = [torch.Tensor([1,1,1,1,1])]
    // optim = torch.optim.SGD(param, lr = 0.1, momentum=.7, nesterov=True)
    // param[0].grad = torch.Tensor([.5, .5, .5, .5, .5])
    // optim.step()
    // print(1-param[0])
    // optim.zero_grad()
    // param[0].grad = torch.Tensor([.1, .2, .3, .4, .5])
    // optim.step()
    // print(0.915-param[0])
    // tensor([0.0850, 0.0850, 0.0850, 0.0850, 0.0850])
    // tensor([0.0415, 0.0585, 0.0755, 0.0925, 0.1095])
    #[tokio::test]
    async fn test_nesterov() {
        use tempfile::TempDir;
        // Create a tmp dir for the test
        let tmp_dir = TempDir::new().unwrap();

        let device = Device::Cpu;
        let gradient_tensor =
            candle_core::Tensor::from_vec(vec![0.5, 0.5, 0.5, 0.5, 0.5], 5, &device).unwrap();
        let gradient_file_name = tmp_dir.path().join("gradient_file");
        safetensors::serialize_to_file(
            vec![("gradient", &gradient_tensor)],
            &None,
            &gradient_file_name.clone(),
        )
        .unwrap();

        let result = nesterov(
            gradient_file_name.clone(),
            tmp_dir.path().to_path_buf(),
            &device,
            0.7,
            0.1,
        )
        .await
        .unwrap();
        let update = candle_core::safetensors::load(result, &device).unwrap();
        assert_eq!(
            update.get("gradient").unwrap().to_vec1::<f64>().unwrap(),
            vec![0.085, 0.085, 0.085, 0.085, 0.085]
        );

        let gradient_tensor =
            candle_core::Tensor::from_vec(vec![0.1, 0.2, 0.3, 0.4, 0.5], 5, &device).unwrap();
        safetensors::serialize_to_file(
            vec![("gradient", &gradient_tensor)],
            &None,
            &gradient_file_name,
        )
        .unwrap();
        let result = nesterov(
            gradient_file_name,
            tmp_dir.path().to_path_buf(),
            &device,
            0.7,
            0.1,
        )
        .await
        .unwrap();
        let update = candle_core::safetensors::load(result, &device).unwrap();
        let difference = update
            .get("gradient")
            .unwrap()
            .to_vec1::<f64>()
            .unwrap()
            .into_iter()
            .zip(vec![0.0415, 0.0585, 0.0755, 0.0925, 0.1095])
            .fold(0f64, |acc, (a, b)| acc + (a - b).abs());
        assert!(difference < 0.000001)
    }
}
