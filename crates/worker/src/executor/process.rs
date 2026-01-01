use std::{future::Future, path::PathBuf, pin::Pin, process::Stdio, time::Duration};

use hypha_messages::Executor;
use hypha_telemetry::otel::KeyValue;
use libp2p::PeerId;
use nix::{
    libc::pid_t,
    sys::signal::{self, Signal},
    unistd::Pid,
};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
    sync::{Mutex, oneshot},
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    config::{Config, ExecutorConfig, ExecutorRuntime},
    connector::Connector,
    executor::{Error, Execution, JobExecutor, Status, bridge::Bridge},
    network::Network,
};

pub struct ProcessExecutor {
    connector: Connector<Network>,
    network: Network,
    work_dir_base: PathBuf,
    executor_config: ExecutorConfig,
    config: Config,
}

pub struct ProcessExecution {
    status_rx: Mutex<Option<oneshot::Receiver<Status>>>,
}

impl ProcessExecution {
    fn new(status_rx: oneshot::Receiver<Status>) -> Self {
        Self {
            status_rx: Mutex::new(Some(status_rx)),
        }
    }
}

impl Execution for ProcessExecution {
    fn wait<'a>(&'a self) -> Pin<Box<dyn Future<Output = Result<Status, Error>> + Send + 'a>> {
        Box::pin(async move {
            let rx = {
                let mut guard = self.status_rx.lock().await;
                guard.take()
            }
            .ok_or_else(|| {
                Error::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "Execution status already consumed",
                ))
            })?;

            match rx.await {
                Ok(status) => Ok(status),
                Err(_) => Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "Execution task ended without status",
                ))),
            }
        })
    }
}

struct RuntimeContext {
    socket_path: String,
    work_dir: String,
    job_json: String,
}

impl RuntimeContext {
    fn new(socket_path: String, work_dir: String, job_json: String) -> Self {
        Self {
            socket_path,
            work_dir,
            job_json,
        }
    }
}

impl ProcessExecutor {
    pub(crate) fn new(
        connector: Connector<Network>,
        network: Network,
        work_dir_base: PathBuf,
        executor_config: ExecutorConfig,
        config: Config,
    ) -> Self {
        ProcessExecutor {
            connector,
            network,
            work_dir_base,
            executor_config,
            config,
        }
    }
}

#[allow(refining_impl_trait)]
impl JobExecutor for ProcessExecutor {
    async fn execute(
        &self,
        job: hypha_messages::JobSpec,
        cancel: CancellationToken,
        _job_id: Uuid,
        scheduler: PeerId,
    ) -> Result<ProcessExecution, Error> {
        if !matches!(&job.executor, Executor::Train(_)) {
            return Err(Error::UnsupportedJobSpec());
        }

        if !matches!(
            self.executor_config.runtime(),
            ExecutorRuntime::Process { .. }
        ) {
            return Err(Error::InvalidExecutorConfig(
                "executor config must be process type".into(),
            ));
        }

        let id = Uuid::new_v4();
        let work_dir = self.work_dir_base.join(format!("hypha-{}", id));
        let sock_path = work_dir.join("bridge.sock");
        fs::create_dir_all(&work_dir).await?;

        let bridge = Bridge::try_new(
            self.connector.clone(),
            self.network.clone(),
            work_dir.clone(),
            sock_path.clone(),
            cancel.clone(),
            job.job_id,
            scheduler,
        )
        .await?;

        let cmd = self.executor_config.cmd().ok_or_else(|| {
            Error::InvalidExecutorConfig("missing command for process executor".into())
        })?;
        let runtime = RuntimeContext::new(
            sock_path.to_string_lossy().into_owned(),
            work_dir.to_string_lossy().into_owned(),
            serde_json::to_string(&job).expect("valid JobSpec JSON"),
        );

        let mut process = Command::new(cmd);
        let args = self
            .executor_config
            .args()
            .iter()
            .map(|arg| replace_placeholders(arg, &runtime))
            .collect::<Vec<_>>();

        process
            .args(args)
            .env("SOCKET_PATH", &runtime.socket_path)
            .env("WORK_DIR", &runtime.work_dir)
            .env("JOB_JSON", &runtime.job_json)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(endpoint) = self.config.telemetry_endpoint() {
            process.env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint.to_string());
        }

        if let Some(headers) = self.config.telemetry_headers() {
            process.env("OTEL_EXPORTER_OTLP_HEADERS", headers.to_string());
        }

        if let Some(attributes) = self.config.telemetry_attributes() {
            process.env("OTEL_RESOURCE_ATTRIBUTES", attributes.to_string());
            // NOTE: Export `service.name` attribute as `OTEL_SERVICE_NAME` env
            //  for processes not yet supporting OTEL attributes.
            let kvs: Vec<KeyValue> = attributes.into();
            if let Some(kv) = kvs.iter().find(|kv| kv.key.as_str() == "service.name") {
                process.env("OTEL_SERVICE_NAME", kv.value.to_string());
            }
        }

        if let Some(protocol) = self.config.telemetry_protocol()
            && let Ok(s) = serde_json::to_string(&protocol)
        {
            process.env("OTEL_EXPORTER_OTLP_PROTOCOL", s.trim_matches('"'));
        }

        if let Some(sampler) = self.config.telemetry_sampler()
            && let Ok(s) = serde_json::to_string(&sampler)
        {
            process.env("OTEL_TRACES_SAMPLER", s.trim_matches('"'));
        }

        if let Some(ratio) = self.config.telemetry_sample_ratio() {
            process.env("OTEL_TRACES_SAMPLER_ARG", ratio.to_string());
        }

        let mut process = process.spawn()?;

        let (status_tx, status_rx) = oneshot::channel();
        let shutdown = cancel.clone();

        tokio::spawn(async move {
            let stdout = process.stdout.take().expect("stdout is available");
            let stderr = process.stderr.take().expect("stderr is available");
            const OUT_LIMIT: usize = 2 * 1024;
            const ERR_LIMIT: usize = 8 * 1024;

            let stdout_handle = tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                let mut buf = String::new();

                while let Ok(Some(line)) = lines.next_line().await {
                    println!("{line}");

                    buf.push_str(&line);
                    buf.push('\n');

                    if buf.len() > OUT_LIMIT {
                        let excess = buf.len() - OUT_LIMIT;
                        buf.drain(..excess);
                    }
                }

                buf
            });

            let stderr_handle = tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut buf = Vec::with_capacity(OUT_LIMIT);
                let mut chunk = [0u8; 1024];

                loop {
                    match reader.read(&mut chunk).await {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);

                            if buf.len() > ERR_LIMIT {
                                let excess = buf.len() - ERR_LIMIT;
                                buf.drain(..excess);
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = ?e, "Failed to read process executor stderr");

                            break;
                        }
                    }
                }

                String::from_utf8_lossy(&buf).to_string()
            });

            let exec_status = tokio::select! {
                status = process.wait() => {
                    match status {
                        Ok(s) => {
                            if s.success() {
                                Status::success(None)
                            } else {
                                Status::failed(Some(format!("Process exited with {}", s)))
                            }
                        }
                        Err(e) => Status::failed(Some(format!("Process wait failed with {}", e))),
                    }
                }
                _ = shutdown.cancelled() => {
                    tracing::debug!("Process executor cancellation received, sending SIGTERM");

                    if let Some(pid) = process.id()
                        && let Err(e) = signal::kill(Pid::from_raw(pid as pid_t), Signal::SIGTERM)
                    {
                        tracing::warn!(error = ?e, "Failed to send SIGTERM to process executor");
                    }

                    match timeout(Duration::from_secs(5), process.wait()).await {
                        Ok(Ok(status)) => {
                            Status::cancelled(Some(format!("Process cancelled and exited with {}", status)))
                        },
                        Ok(Err(e)) => {
                            Status::failed(Some(format!("Process exited with error {}", e)))
                        },
                        Err(_) => {
                            tracing::warn!("Process executor didn't exit in time, sending SIGKILL");

                            match process.kill().await {
                                Ok(_) => Status::cancelled(Some("Process force killed".into())),
                                Err(e) => Status::failed(Some(
                                    format!("Process force kill failed with {}", e)
                                ))
                            }
                        }
                    }
                }
            };

            // NOTE: Wait for the process to exit before reading its output and error streams,
            // then add them to the status output if not empty to improve error reporting.
            let out = stdout_handle.await.unwrap_or_default();
            let err = stderr_handle.await.unwrap_or_default();

            let _ = status_tx.send({
                let out_val = (!out.is_empty()).then_some(out);
                let err_val = (!err.is_empty()).then_some(err);

                match exec_status {
                    Status::Success { description, .. } => Status::Success {
                        description,
                        out: out_val,
                        err: err_val,
                    },
                    Status::Failed { description, .. } => Status::Failed {
                        description,
                        out: out_val,
                        err: err_val,
                    },
                    Status::Cancelled { description, .. } => Status::Cancelled {
                        description,
                        out: out_val,
                        err: err_val,
                    },
                    Status::Running => Status::Running,
                }
            });

            shutdown.cancel();
            let _ = bridge.wait().await;
            let _ = fs::remove_file(&sock_path).await;
            let _ = fs::remove_dir_all(&work_dir).await;
        });

        Ok(ProcessExecution::new(status_rx))
    }
}

fn replace_placeholders(arg: &str, ctx: &RuntimeContext) -> String {
    arg.replace("{SOCKET_PATH}", &ctx.socket_path)
        .replace("{WORK_DIR}", &ctx.work_dir)
        .replace("{JOB_JSON}", &ctx.job_json)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod replace_placeholders {
        use super::RuntimeContext;

        #[test]
        fn substitutes_all_known_tokens() {
            let ctx = RuntimeContext::new(
                "/tmp/socket".into(),
                "/tmp/work".into(),
                "{\"id\":1}".into(),
            );
            let arg = "run --socket {SOCKET_PATH} --dir {WORK_DIR} --job {JOB_JSON}";

            let replaced = super::replace_placeholders(arg, &ctx);

            assert_eq!(
                replaced,
                "run --socket /tmp/socket --dir /tmp/work --job {\"id\":1}"
            );
        }

        #[test]
        fn leaves_unknown_tokens_intact() {
            let ctx = RuntimeContext::new("socket".into(), "work".into(), "job".into());
            let arg = "noop {UNKNOWN} literal";

            let replaced = super::replace_placeholders(arg, &ctx);

            assert_eq!(replaced, arg);
        }
    }
}
