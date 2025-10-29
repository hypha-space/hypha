use std::{collections::HashMap, path::PathBuf, sync::Arc};

use hypha_messages::{Executor, ExecutorDescriptor, JobSpec, JobStatus};
use hypha_network::request_response::RequestResponseError;
use libp2p::PeerId;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    config::{ExecutorConfig, ExecutorRuntime},
    connector::Connector,
    executor::{self, Execution, JobExecutor, ParameterServerExecutor, ProcessExecutor},
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
    pub id: Uuid,
    pub lease: Uuid,
    pub scheduler: PeerId,
    pub spec: JobSpec,
    pub cancel_token: CancellationToken,
    pub execution: Box<dyn Execution + Sync + Send>,
}

impl Job {
    pub async fn cancel(&self) -> Result<(), JobManagerError> {
        tracing::debug!(job_id = %self.id, "Cancelling job");
        self.cancel_token.cancel();
        self.execution.wait().await;
        Ok(())
    }
}

#[derive(Clone)]
pub struct JobManager {
    active_jobs: Arc<Mutex<HashMap<Uuid, Job>>>,
    connector: Connector<Network>,
    network: Network,
    work_dir_base: PathBuf,
    executor_configs: Vec<ExecutorConfig>,
}

impl JobManager {
    pub fn new(
        connector: Connector<Network>,
        network: Network,
        work_dir_base: PathBuf,
        executor_configs: Vec<ExecutorConfig>,
    ) -> Self {
        Self {
            active_jobs: Arc::new(Mutex::new(HashMap::new())),
            connector,
            network,
            work_dir_base,
            executor_configs,
        }
    }

    fn find_executor_config(&self, descriptor: &ExecutorDescriptor) -> Option<&ExecutorConfig> {
        self.executor_configs
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
                );
                let execution = executor
                    .execute(spec.clone(), cancel_token.clone(), spec.job_id, scheduler)
                    .await?;
                let job = Job {
                    id,
                    lease,
                    scheduler,
                    spec: spec.clone(),
                    cancel_token: cancel_token.clone(),
                    execution: Box::new(execution),
                };
                self.active_jobs.lock().await.insert(id, job);
                Ok(())
            }
            Executor::Aggregate(_) => {
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
                let job = Job {
                    id,
                    lease,
                    scheduler,
                    spec: spec.clone(),
                    cancel_token: cancel_token.clone(),
                    execution: Box::new(execution),
                };
                self.active_jobs.lock().await.insert(id, job);
                Ok(())
            }
        }
    }

    pub async fn cancel(&mut self, job_id: &Uuid) -> Result<(), JobManagerError> {
        let job = {
            let mut guard = self.active_jobs.lock().await;
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

    pub async fn find_jobs_by_lease(&self, lease_id: &Uuid) -> Vec<Uuid> {
        self.active_jobs
            .lock()
            .await
            .values()
            .filter_map(|job| (job.lease == *lease_id).then_some(job.id))
            .collect()
    }

    pub async fn find_jobs_by_scheduler(&self, scheduler: &PeerId) -> Vec<Uuid> {
        self.active_jobs
            .lock()
            .await
            .values()
            .filter_map(|job| (job.scheduler == *scheduler).then_some(job.id))
            .collect()
    }

    pub async fn get_status(&self, job_id: &Uuid) -> Result<JobStatus, JobManagerError> {
        Err(JobManagerError::TaskNotFound(*job_id))
    }

    pub async fn cleanup_finished_jobs(&mut self) -> Vec<Uuid> {
        Vec::new()
    }

    pub async fn shutdown(&mut self) {
        let jobs: Vec<Job> = {
            let mut guard = self.active_jobs.lock().await;
            guard.drain().map(|(_, job)| job).collect()
        };

        for job in jobs.iter() {
            let _ = job.cancel().await;
        }
    }
}
