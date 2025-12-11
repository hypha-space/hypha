use std::{collections::HashMap, path::PathBuf, sync::Arc};

use hypha_messages::{Executor, ExecutorDescriptor, JobSpec};
use hypha_network::request_response::RequestResponseError;
use libp2p::PeerId;
use thiserror::Error;
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    config::{Config, ExecutorConfig, ExecutorRuntime},
    connector::Connector,
    executor::{self, Execution, JobExecutor, ParameterServerExecutor, ProcessExecutor, Status},
    network::Network,
};

#[derive(Debug, Error)]
pub enum JobManagerError {
    #[error("Executor error: {0}")]
    Executor(#[source] std::io::Error),
    #[error("Network error: {0}")]
    Network(#[from] RequestResponseError),
    #[error("Task not found: {0}")]
    TaskNotFound(Uuid),
    #[error("Invalid job specification")]
    InvalidJobSpec,
    #[error("Execution error: {0}")]
    Execution(#[from] executor::Error),
    #[error("Executor not supported")]
    ExecutorNotSupported,
    #[error("Executor configuration missing: {0:?}")]
    ExecutorConfigMissing(ExecutorDescriptor),
}

pub struct Job {
    id: Uuid,
    lease: Uuid,
    scheduler: PeerId,
    spec: JobSpec,
    cancel_token: CancellationToken,
    execution: Box<dyn Execution + Sync + Send>,
    status_rx: watch::Receiver<Status>,
}

impl Job {
    fn status(&self) -> Status {
        self.status_rx.borrow().clone()
    }

    pub async fn cancel(&self) -> Result<(), JobManagerError> {
        tracing::debug!(job_id = %self.id, "Cancelling job");
        self.cancel_token.cancel();
        let _ = self.execution.wait().await;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct JobDescriptor {
    pub id: Uuid,
    pub lease: Uuid,
    pub scheduler: PeerId,
    pub spec: JobSpec,
    pub status: Status,
}

impl From<&Job> for JobDescriptor {
    fn from(job: &Job) -> Self {
        JobDescriptor {
            id: job.id,
            lease: job.lease,
            scheduler: job.scheduler,
            spec: job.spec.clone(),
            status: job.status(),
        }
    }
}

#[derive(Clone)]
pub struct JobManager {
    jobs: Arc<Mutex<HashMap<Uuid, Job>>>,
    connector: Connector<Network>,
    network: Network,
    work_dir_base: PathBuf,
    config: Config,
}

impl JobManager {
    pub fn new(
        connector: Connector<Network>,
        network: Network,
        work_dir_base: PathBuf,
        config: Config,
    ) -> Self {
        Self {
            jobs: Arc::new(Mutex::new(HashMap::new())),
            connector,
            network,
            work_dir_base,
            config,
        }
    }

    fn find_executor_config(&self, descriptor: &ExecutorDescriptor) -> Option<&ExecutorConfig> {
        self.config
            .executors()
            .iter()
            .find(|cfg| cfg.descriptor() == *descriptor)
    }

    pub async fn execute(
        &mut self,
        id: Uuid,
        spec: JobSpec,
        lease: Uuid,
        scheduler: PeerId,
    ) -> Result<(), JobManagerError> {
        let cancel_token = CancellationToken::new();
        tracing::info!(job_id = %id, "Job dispatched for execution");

        match &spec.executor {
            Executor::Train(_) => {
                let (status_tx, status_rx) = watch::channel(Status::Running);
                let descriptor = ExecutorDescriptor::from(&spec.executor);
                let config = self
                    .find_executor_config(&descriptor)
                    .cloned()
                    .ok_or_else(|| JobManagerError::ExecutorConfigMissing(descriptor.clone()))?;

                if !matches!(config.runtime(), ExecutorRuntime::Process { .. }) {
                    return Err(JobManagerError::ExecutorNotSupported);
                }

                let executor = ProcessExecutor::new(
                    self.connector.clone(),
                    self.network.clone(),
                    self.work_dir_base.clone(),
                    config,
                    self.config.clone(),
                );
                let execution = executor
                    .execute(spec.clone(), cancel_token.clone(), spec.job_id, scheduler)
                    .await?;

                let execution_monitor = execution.clone();

                tokio::spawn(async move {
                    let status = match execution_monitor.wait().await {
                        Ok(s) => s,
                        Err(e) => Status::Failed(e.to_string()),
                    };

                    tracing::info!("Job completed with status: {:?}", status);

                    status_tx.send(status)
                });

                let job = Job {
                    id,
                    lease,
                    scheduler,
                    spec: spec.clone(),
                    cancel_token: cancel_token.clone(),
                    execution: Box::new(execution),
                    status_rx,
                };

                self.jobs.lock().await.insert(id, job);

                Ok(())
            }
            Executor::Aggregate(_) => {
                let (status_tx, status_rx) = watch::channel(Status::Running);
                let descriptor = ExecutorDescriptor::from(&spec.executor);
                let config = self
                    .find_executor_config(&descriptor)
                    .cloned()
                    .ok_or_else(|| JobManagerError::ExecutorConfigMissing(descriptor.clone()))?;

                if !matches!(config.runtime(), ExecutorRuntime::ParameterServer) {
                    return Err(JobManagerError::ExecutorNotSupported);
                }

                let executor = ParameterServerExecutor::new(
                    self.connector.clone(),
                    self.network.clone(),
                    self.work_dir_base.clone(),
                );
                let execution = executor
                    .execute(spec.clone(), cancel_token.clone(), spec.job_id, scheduler)
                    .await?;

                let execution_monitor = execution.clone();

                tokio::spawn(async move {
                    let status = match execution_monitor.wait().await {
                        Ok(s) => s,
                        Err(e) => Status::Failed(e.to_string()),
                    };

                    tracing::info!("Job completed with status: {:?}", status);

                    status_tx.send(status)
                });

                let job = Job {
                    id,
                    lease,
                    scheduler,
                    spec: spec.clone(),
                    cancel_token: cancel_token.clone(),
                    execution: Box::new(execution),
                    status_rx,
                };

                self.jobs.lock().await.insert(id, job);

                Ok(())
            }
        }
    }

    pub async fn cancel(&mut self, job_id: &Uuid) -> Result<(), JobManagerError> {
        let job = {
            let mut guard = self.jobs.lock().await;
            guard.remove(job_id)
        };

        match job {
            Some(job) => {
                job.cancel().await?;
                Ok(())
            }
            None => Err(JobManagerError::TaskNotFound(*job_id)),
        }
    }

    pub async fn find_jobs_where<F>(&self, mut predicate: F) -> Vec<JobDescriptor>
    where
        F: FnMut(&JobDescriptor) -> bool,
    {
        self.jobs
            .lock()
            .await
            .values()
            .filter_map(|job| {
                let descriptor = job.into();
                predicate(&descriptor).then_some(descriptor)
            })
            .collect()
    }

    pub async fn shutdown(&mut self) {
        let jobs: Vec<Job> = {
            let mut guard = self.jobs.lock().await;
            guard.drain().map(|(_, job)| job).collect()
        };

        for job in jobs.iter() {
            let _ = job.cancel().await;
        }
    }
}
