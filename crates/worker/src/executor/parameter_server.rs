use std::{
    collections::HashMap, fs::Permissions, os::unix::fs::PermissionsExt, path::PathBuf, pin::Pin,
};

use candle_core::{
    Device,
    safetensors::{Load, MmapedSafetensors},
};
use futures::StreamExt;
use safetensors::serialize_to_file;
use tokio::{fs, sync::mpsc};
use tokio::io::{AsyncReadExt as TokioAsyncReadExt, AsyncWriteExt as TokioAsyncWriteExt};
use tokio_util::{
    compat::{FuturesAsyncReadCompatExt, FuturesAsyncWriteCompatExt},
    sync::CancellationToken,
    task::TaskTracker,
};
use tracing::instrument::WithSubscriber;
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
        let task_tracker_clone = task_tracker.clone();
        task_tracker.spawn(async move {
            let fut = async {
                let mut result_tensor: Option<(PathBuf, MmapedSafetensors)> = None;

                let num_workers = updates.get_peers().len();
                let mut current_worker = 0;

                // NOTE: Receive streams in parallel, but keep processing (averaging + broadcasting)
                // sequential to stay within memory constraints and preserve existing logic.
                let mut incoming = match connector.receive(updates).await {
                    Ok(s) => s,
                    Err(_) => return,
                };

                // Channel to report finished file paths from parallel receivers to the sequenced processor.
                let (tx, mut rx) = mpsc::channel::<(String, PathBuf)>(num_workers.max(1) * 2);

                // Spawn a task to accept incoming streams and spawn per-stream copy tasks.
                let work_dir_accept = work_dir.clone();
                tokio::spawn(async move {
                    while let Some(item) = incoming.next().await.transpose().ok().flatten() {
                    let name = item.meta.name.clone();
                        tracing::info!(file_name = ?name, "Received parameter server update (start)");
                        let mut reader = item.reader.compat();
                        let file_name = work_dir_accept.join(name.clone());
                        let tx = tx.clone();

                        // Spawn the copy task; stream data directly to disk without buffering fully in memory.
                        tokio::spawn(async move {
                            match fs::File::create(&file_name).await {
                                Ok(mut file) => {
                                    // Try auto-detect framed (chunked) vs raw stream and copy accordingly.
                                    match recv_auto(&mut reader, &mut file).await {
                                        Ok(received) => tracing::info!(
                                            received_bytes = received,
                                            path = %file_name.display(),
                                            from = %name,
                                            "Parameter server received update"
                                        ),
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
                        });
                    }
                });

                // Sequentially process completed files as they arrive.
                while let Some((_name, file_name)) = rx.recv().await {
                    // NOTE: Prefer continuing with the last good aggregation if averaging fails.
                    // Borrow current aggregation state to avoid moving out of `result_tensor`.
                    let result_tensor_file_name = match result_tensor.as_ref() {
                        None => file_name,
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

                            Ok(writers) => {
                                tracing::info!("Obtained writers");

                                writers.for_each_concurrent(None,  | item_res| async {

                                    match item_res {
                                        Ok(item) => {
                                                tracing::info!("Sending update to peer, meta {:?}", item.meta);
                                            if let Some((ref result_file_name, _)) = result_tensor {
                                                match fs::File::open(result_file_name.as_path()).await {
                                                    Ok(mut file) => {

                                                        tracing::info!("Opening file, meta {:?}, file: {:?}", item.meta, file.metadata().await);

                                                        // NOTE: Measure exact bytes sent and compare to source length for diagnostics.
                                                        let expected_len = match file.metadata().await {
                                                            Ok(meta) => meta.len(),
                                                            Err(_) => 0,
                                                        };
                                                        let mut writer = item.writer.compat_write();
                                                        // Send in 10MiB framed chunks with a magic header.
                                                        let sent = match send_in_chunks(&mut file, &mut writer).await {
                                                            Ok(n) => n,
                                                            Err(e) => {
                                                                tracing::warn!(error = ?e, "Failed to write update to peer");
                                                                0
                                                            }
                                                        };

                                                        // Cleanly finish the stream so the receiver knows we're done.
                                                        if let Err(e) = TokioAsyncWriteExt::flush(&mut writer).await {
                                                            tracing::warn!(error = ?e, "Failed to flush writer to peer");
                                                        }
                                                        if let Err(e) = TokioAsyncWriteExt::shutdown(&mut writer).await {
                                                            tracing::warn!(error = ?e, "Failed to shutdown writer to peer");
                                                        }

                                                        if sent != expected_len {
                                                            tracing::warn!(
                                                                expected_bytes = expected_len,
                                                                sent_bytes = sent,
                                                                peer = %item.meta.name,
                                                                "Parameter server sent byte mismatch"
                                                            );
                                                        }
                                                        tracing::info!(
                                                            expected_bytes = expected_len,
                                                            sent_bytes = sent,
                                                            peer = %item.meta.name,
                                                            "Finished sending update to peer"
                                                        );
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
                                }).await;
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

            // Clean up.
            let _ = fs::remove_dir_all(&work_dir).await;
        });

        task_tracker.close();

        Ok(ParameterServerExecution { task_tracker })
    }
}

// NOTE: Simple framed transfer helpers (10 MiB chunks) with a magic prelude for compatibility.
const CHUNK_SIZE: usize = 10 * 1024 * 1024; // 10 MiB
const MAX_CHUNK: usize = 64 * 1024 * 1024; // safety cap for headers
const MAGIC: &[u8; 4] = b"HPCH"; // Hypha Chunk framing

async fn send_in_chunks<R, W>(r: &mut R, w: &mut W) -> std::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut total: u64 = 0;
    let mut buf = vec![0u8; CHUNK_SIZE];
    // Write magic header once per stream
    w.write_all(MAGIC).await?;
    let mut idx: u64 = 0;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let len = (n as u64).to_le_bytes();
        w.write_all(&len).await?;
        w.write_all(&buf[..n]).await?;
        total += n as u64;
        tracing::info!(chunk_index = idx, chunk_bytes = n, total_sent = total, "Sent chunk");
        idx += 1;
    }
    Ok(total)
}

async fn recv_auto<R, W>(r: &mut R, w: &mut W) -> std::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut total: u64 = 0;
    let mut magic = [0u8; 4];
    // Try to read magic; handle EOF gracefully
    match read_exact_bytes(r, &mut magic).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(0),
        Err(e) => return Err(e),
    }
    if &magic == MAGIC {
        let mut hdr = [0u8; 8];
        let mut buf = vec![0u8; CHUNK_SIZE.max(1)];
        let mut idx: u64 = 0;
        loop {
            // Next header; EOF here means done
            match read_exact_bytes(r, &mut hdr).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            }
            let len = u64::from_le_bytes(hdr) as usize;
            if len == 0 || len > MAX_CHUNK {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid chunk len {}", len),
                ));
            }
            if buf.len() < len {
                buf.resize(len, 0);
            }
            read_exact_bytes(r, &mut buf[..len]).await?;
            TokioAsyncWriteExt::write_all(w, &buf[..len]).await?;
            total += len as u64;
            tracing::info!(chunk_index = idx, chunk_bytes = len, total_received = total, "Received chunk");
            idx += 1;
        }
        Ok(total)
    } else {
        // Framing is required: reject non-framed streams
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing HPCH magic header",
        ));
    }
}

async fn read_exact_bytes<R: tokio::io::AsyncRead + Unpin>(r: &mut R, mut buf: &mut [u8]) -> std::io::Result<()> {
    while !buf.is_empty() {
        match TokioAsyncReadExt::read(r, buf).await? {
            0 => return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "early EOF")),
            n => {
                let tmp = buf;
                buf = &mut tmp[n..];
            }
        }
    }
    Ok(())
}
