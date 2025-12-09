use std::{
    collections::{HashMap, HashSet},
    fs::Permissions,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::SystemTime,
};

use candle_core::{
    Device, Tensor,
    safetensors::{Load, MmapedSafetensors},
};
use futures_util::StreamExt;
use hypha_messages::{
    Executor, Nesterov, Receive, Send as SendRef,
    action::{self, AggregateError, ExecutorStatus},
};
use libp2p::PeerId;
use safetensors::serialize_to_file;
use sha2::{Digest, Sha256};
use tokio::{
    fs,
    io::{self, AsyncWriteExt},
    sync::{Mutex, Notify},
};
use tokio_retry::{
    Retry,
    strategy::{ExponentialBackoff, jitter},
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
    network: Network,
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
    pub fn new(connector: Connector<Network>, network: Network, work_dir_base: PathBuf) -> Self {
        ParameterServerExecutor {
            connector,
            network,
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
        job_id: Uuid,
        scheduler_id: PeerId,
    ) -> Result<ParameterServerExecution, Error> {
        tracing::info!(job_spec = ?job, "Executing parameter server job");

        let retry_strategy = ExponentialBackoff::from_millis(100)
            .map(jitter) // add jitter to delays
            .take(3); // limit to 3 retries

        let id = Uuid::new_v4();
        let work_dir = self.work_dir_base.join(format!("hypha-{}", id));
        fs::create_dir_all(&work_dir).await?;

        let device = Device::Cpu;

        let optimizer = match &job.executor {
            Executor::Aggregate(aggregate) => {
                let config = aggregate.config().clone();
                config.optimizer
            }
            _ => return Err(Error::UnsupportedJobSpec()),
        };

        let connector = self.connector.clone();
        let network = self.network.clone();

        let task_tracker = TaskTracker::new();

        let incoming_dir = work_dir.join("incoming");
        fs::create_dir_all(&incoming_dir).await?;
        let updates_store: Arc<Mutex<HashMap<PeerId, Vec<PathBuf>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let updates_notify = Arc::new(Notify::new());

        {
            let connector = self.connector.clone();
            let updates_store = updates_store.clone();
            let updates_notify = updates_notify.clone();
            let incoming_dir = incoming_dir.clone();
            let cancel = cancel.clone();
            task_tracker.spawn(async move {
                let receive_any = Receive::peers(Vec::new());
                let mut incoming = match connector.receive(receive_any).await {
                    Ok(s) => s,
                    Err(err) => {
                        tracing::error!(error = %err, "Receiver failed to start");
                        return;
                    }
                };

                loop {
                    let next_item = tokio::select! {
                        _ = cancel.cancelled() => None,
                        item = incoming.next() => item,
                    };
                    let Some(item_result) = next_item else { break; };
                    let item = match item_result {
                        Ok(it) => it,
                        Err(err) => {
                            tracing::error!(error = %err, "Receiver stream error");
                            break;
                        }
                    };
                    let peer = item.meta.name.clone();
                    let peer_dir = incoming_dir.join(&peer);
                    if let Err(e) = fs::create_dir_all(&peer_dir).await {
                        tracing::error!(error = %e, dir = %peer_dir.display(), "Failed to create peer staging dir");
                        continue;
                    }
                    let file_path = peer_dir.join(format!("{}.pt", Uuid::new_v4()));
                    let mut reader = item.reader;
                    match fs::File::create(&file_path).await {
                        Ok(mut f) => {
                            match io::copy(&mut reader, &mut f).await {
                                Ok(n) => {
                                    let _ = f.sync_all().await;
                                    let _ = fs::set_permissions(&file_path, Permissions::from_mode(0o600)).await;
                                    tracing::debug!(peer_id = %peer, size = n, file = %file_path.display(), "Received update");                                }
                                Err(err) => {
                                    tracing::error!(error = %err, file = %file_path.display(), "Failed to write received update");
                                }
                            }
                        }
                        Err(err) => {
                            tracing::error!(error = %err, file = %file_path.display(), "Failed to create staging file");
                        }
                    }

                    {
                        let mut store = updates_store.lock().await;
                        let pid = peer.parse().unwrap_or_else(|_| PeerId::random());
                        let entry = store.entry(pid).or_default();
                        entry.push(file_path.clone());
                    }
                    updates_notify.notify_one();
                }
            });
        }

        task_tracker.spawn({
            let retry_strategy = retry_strategy.clone();
            async move {
                let mut current_status = ExecutorStatus::Aggregate(action::AggregateStatus::Idle);
                let mut pending_update: Option<PathBuf> = None;

                loop {
                    tracing::debug!(job_id = %job_id, status = ?current_status, "Requesting next aggregate action");

                    let action_response = match Retry::spawn(retry_strategy.clone(), || {
                        let status = current_status.clone();
                        let network = network.clone();
                        async move {
                            hypha_network::request_response::RequestResponseInterface::<
                                action::Codec,
                            >::request(
                                &network,
                                scheduler_id,
                                action::ActionRequest { job_id, status },
                            )
                            .await
                        }
                    })
                    .await
                    {
                        Ok(resp) => resp,
                        Err(e) => {
                            tracing::warn!(job_id = %job_id, error = %e, status = ?current_status, "Failed to fetch next aggregate action");
                            current_status = ExecutorStatus::Aggregate(action::AggregateStatus::Error(AggregateError::Connection  {
                                    message: format!("action request failed: {e}"),
                                },
                            ));
                            continue;
                        }
                    };

                    tracing::debug!(job_id = %job_id, next = ?action_response.next,
                        "Received aggregate action");

                    match action_response.next {
                        action::ExecutorAction::Aggregate(agg_action) => match agg_action {
                            action::AggregateAction::Terminate => break,
                            action::AggregateAction::Idle { timeout } => {
                                if let Ok(duration) = timeout.duration_since(SystemTime::now()) {
                                    // Wake early if new updates arrive; otherwise wait for timeout.
                                    tokio::select! {
                                        _ = cancel.cancelled() => break,
                                        _ = updates_notify.notified() => {},
                                        _ = tokio::time::sleep(duration) => {},
                                    }
                                }
                                current_status =
                                    ExecutorStatus::Aggregate(action::AggregateStatus::Idle);
                            }
                            action::AggregateAction::AggregateUpdates { source } => {
                                let receive = match Receive::try_from(source) {
                                    Ok(r) => r,
                                    Err(e) => {
                                        tracing::warn!(error = %e, "Invalid receive reference");
                                        current_status = ExecutorStatus::Aggregate(
                                            action::AggregateStatus::Error(AggregateError::Connection {
                                                message: e.to_string(),
                                            }),
                                        );
                                        continue;
                                    }
                                };

                                // NOTE: Allowed peers come from scheduler. If empty, accept any.
                                let allowed = receive.get_peers().clone();
                                // TODO: These should come from the scheduler and must be configurable.
                                let max_delay = std::time::Duration::from_millis(500);
                                let action_deadline = std::time::Duration::from_secs(30);

                                match aggregate_updates(
                                    updates_store.clone(),
                                    allowed,
                                    work_dir.clone(),
                                    &device,
                                    &optimizer,
                                    max_delay,
                                    action_deadline,
                                    updates_notify.clone(),
                                    cancel.clone(),
                                )
                                .await
                                {
                                    Ok(path) => {
                                        pending_update = Some(path);
                                        current_status = ExecutorStatus::Aggregate(
                                            action::AggregateStatus::AggregatedUpdates {
                                                metrics: None,
                                            },
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "Failed to aggregate updates");
                                        current_status = ExecutorStatus::Aggregate(
                                            action::AggregateStatus::Error(AggregateError::Other {
                                                message: e.to_string(),
                                            }),
                                        );
                                    }
                                }
                            }
                            action::AggregateAction::BroadcastUpdate { target } => {
                                let Some(ref gradient_file) = pending_update else {
                                    current_status = ExecutorStatus::Aggregate(
                                        action::AggregateStatus::Error(AggregateError::Other {
                                            message: "no aggregated update available".into(),
                                        },
                                    ));

                                    continue;
                                };

                                let send = match SendRef::try_from(target) {
                                    Ok(s) => s,
                                    Err(e) => {
                                        tracing::warn!(error = %e, "Invalid send reference");
                                        current_status = ExecutorStatus::Aggregate(
                                            action::AggregateStatus::Error(AggregateError::Connection {
                                                message: e.to_string(),
                                            },
                                        ));

                                        continue;
                                    }
                                };

                                match broadcast_update(
                                    connector.clone(),
                                    send,
                                    gradient_file,
                                    cancel.clone(),
                                )
                                .await
                                {
                                    Ok(()) => {
                                        pending_update = None;
                                        current_status = ExecutorStatus::Aggregate(
                                            action::AggregateStatus::BroadcastedUpdate {
                                                metrics: None,
                                            },
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "Failed to broadcast update");
                                        current_status = ExecutorStatus::Aggregate(
                                            action::AggregateStatus::Error(AggregateError::Connection {
                                                message: e.to_string(),
                                            },
                                        ));
                                    }
                                }
                            }
                        },
                        other => {
                            tracing::warn!(
                                ?other,
                                "Received unexpected action for parameter server"
                            );
                            current_status = ExecutorStatus::Aggregate(action::AggregateStatus::Error(AggregateError::Other {
                                    message: "unexpected executor action".into(),
                                },
                            ));
                        }
                    }

                    if cancel.is_cancelled() {
                        break;
                    }
                }

                let _ = fs::remove_dir_all(&work_dir).await;
            }
        });

        task_tracker.close();

        Ok(ParameterServerExecution { task_tracker })
    }
}

// NOTE: Aggregation requires many contextual parameters (store, peers, timers, etc.); we keep
// them explicit to not hide important dependencies behind structs.
#[allow(clippy::too_many_arguments)]
async fn aggregate_updates(
    store: Arc<Mutex<HashMap<PeerId, Vec<PathBuf>>>>,
    allowed_peers: Vec<PeerId>,
    work_dir: PathBuf,
    device: &Device,
    optimizer: &Nesterov,
    gap_timeout: std::time::Duration,
    action_deadline: std::time::Duration,
    notify: Arc<Notify>,
    cancel: CancellationToken,
) -> Result<PathBuf, Error> {
    let allowed: HashSet<PeerId> = allowed_peers.into_iter().collect();
    let mut used: HashSet<PeerId> = HashSet::new();
    let deadline = tokio::time::Instant::now() + action_deadline;

    // NOTE: Max delay we allow for any peer to send an update
    // when, if have not received an update within this time, we end the action.
    let max_delay = tokio::time::sleep(gap_timeout);
    tokio::pin!(max_delay);

    let mut current_result_tensor_file_name: Option<PathBuf> = None;

    loop {
        let maybe_update = {
            let mut guard = store.lock().await;

            // NOTE: Determine eligible peers: if allowed empty, accept any; else only allowed
            let keys: Vec<PeerId> = if allowed.is_empty() {
                guard.keys().cloned().collect()
            } else {
                guard
                    .keys()
                    .filter(|p| allowed.contains(p))
                    .cloned()
                    .collect()
            };

            // NOTE: Pick first with available files that we haven't used yet in this round
            let mut chosen: Option<(PeerId, PathBuf)> = None;
            for pid in keys {
                if used.contains(&pid) {
                    continue;
                }
                if let Some(files) = guard.get_mut(&pid)
                    && let Some(path) = files.pop()
                {
                    chosen = Some((pid, path));
                }
                if chosen.is_some() {
                    break;
                }
            }
            chosen
        };

        if let Some((peer_id, file_name)) = maybe_update {
            tracing::info!(peer_id = %peer_id, file = %file_name.display(),
                "Aggregating received update");
            used.insert(peer_id);

            // NOTE: Merge into running average
            current_result_tensor_file_name = match current_result_tensor_file_name {
                None => Some(file_name.to_path_buf()),
                Some(result_tensor_file_name) => {
                    let resulting_tensor_file_name =
                        work_dir.join(format!("joined_{:?}", Uuid::new_v4()));
                    let sum_op = |a: &Tensor, b: &Tensor| a + b;
                    apply_tensor_op(
                        &file_name,
                        &result_tensor_file_name,
                        &resulting_tensor_file_name,
                        &work_dir,
                        device,
                        sum_op,
                    )
                    .await?;
                    let _ = fs::remove_file(&file_name).await;
                    let _ = fs::remove_file(&result_tensor_file_name).await;
                    Some(resulting_tensor_file_name)
                }
            };

            // NOTE: Reset gap timer after receiving an update
            max_delay
                .as_mut()
                .reset(tokio::time::Instant::now() + gap_timeout);

            // If we've used all allowed peers, we can stop
            if !allowed.is_empty() && used.len() >= allowed.len() {
                break;
            }
            continue;
        }

        // NOTE: No update immediately available: wait for either gap timeout, new update, deadline, or cancel
        tokio::select! {
            _ = cancel.cancelled() => return Err(Error::InvalidExecutorConfig("aggregation cancelled".to_string())),
            _ = &mut max_delay => {
                tracing::debug!("Aggregate max delay reached");
                break;
            },
            _ = tokio::time::sleep_until(deadline) => {
                tracing::warn!("Aggregate deadline reached");
                break;
            },
            _ = notify.notified() => {
                // New updates available; loop to try again
                continue;
            }
        }
    }

    let final_tensor_file_name = work_dir.join("avg-final");
    if let Some(result_tensor_file_name) = current_result_tensor_file_name.as_ref() {
        fs::rename(
            result_tensor_file_name.as_path(),
            final_tensor_file_name.as_path(),
        )
        .await?;
    } else {
        return Err(Error::InvalidExecutorConfig(
            "no updates available to aggregate".to_string(),
        ));
    }

    let gradient_file = nesterov(
        final_tensor_file_name.clone(),
        work_dir.clone(),
        device,
        optimizer.momentum,
        optimizer.learning_rate,
    )
    .await?;

    let _ = fs::remove_file(final_tensor_file_name.as_path()).await;

    Ok(gradient_file)
}

async fn broadcast_update(
    connector: Connector<Network>,
    send: SendRef,
    gradient_file: &Path,
    cancel: CancellationToken,
) -> Result<(), Error> {
    let mut writers = connector.send(send).await?;

    loop {
        let next_item = tokio::select! {
            _ = cancel.cancelled() => None,
            item = writers.next() => item,
        };

        let Some(item_result) = next_item else {
            break;
        };
        let item = item_result?;
        tracing::info!(peer_id = item.meta.name, "Sending parameter server update");
        let mut reader = fs::File::open(gradient_file).await?;
        let mut writer = item.writer;
        io::copy(&mut reader, &mut writer).await?;
        writer.shutdown().await?;
    }

    let _ = fs::remove_file(gradient_file).await;

    Ok(())
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
