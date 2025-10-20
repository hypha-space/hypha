use std::{collections::HashMap, path::PathBuf, sync::Arc};

use hypha_messages::{Executor, JobSpec, JobStatus};
use hypha_network::request_response::RequestResponseError;
use libp2p::PeerId;
use thiserror::Error;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
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
}

pub struct Job {
    pub id: Uuid,
    pub lease: Uuid,
    pub scheduler: PeerId,
    pub spec: JobSpec,
    pub cancel_token: CancellationToken,
    // NOTE: Type-erased execution handle to support heterogeneous executors.
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

/// Manages job execution and lifecycle
#[derive(Clone)]
pub struct JobManager {
    active_jobs: Arc<Mutex<HashMap<Uuid, Job>>>,
    connector: Connector<Network>,
    work_dir_base: PathBuf,
}

impl JobManager {
    pub fn new(connector: Connector<Network>, work_dir_base: PathBuf) -> Self {
        Self {
            active_jobs: Arc::new(Mutex::new(HashMap::new())),
            connector,
            work_dir_base,
        }
    }

    /// Dispatch a new job for execution
    pub async fn execute(
        &mut self,
        id: Uuid,
        spec: JobSpec,
        lease: Uuid,
        scheduler: PeerId,
    ) -> Result<(), JobManagerError> {
        let cancel_token = CancellationToken::new();
        tracing::info!(
            job_id = %id,
            "Job dispatched for execution"
        );

        // NOTE: Spawn job execution based on executor type
        match &spec.executor {
            Executor::DiLoCoTransformer { .. } => {
                let executor =
                    ProcessExecutor::new(self.connector.clone(), self.work_dir_base.clone());
                let execution = executor.execute(spec.clone(), cancel_token.clone()).await?;
                let job = Job {
                    id,
                    lease,
                    scheduler,
                    spec: spec.clone(),
                    cancel_token: cancel_token.clone(),
                    // NOTE: Box the concrete execution handle as a trait object.
                    execution: Box::new(execution),
                };
                self.active_jobs.lock().await.insert(id, job);

                Ok(())
            }
            Executor::ParameterServer { .. } => {
                let executor = ParameterServerExecutor::new(
                    self.connector.clone(),
                    self.work_dir_base.clone(),
                );
                let execution = executor.execute(spec.clone(), cancel_token.clone()).await?;
                let job = Job {
                    id,
                    lease,
                    scheduler,
                    spec: spec.clone(),
                    cancel_token: cancel_token.clone(),
                    // NOTE: Box the concrete execution handle as a trait object.
                    execution: Box::new(execution),
                };
                self.active_jobs.lock().await.insert(id, job);

                Ok(())
            }
            _ => Err(JobManagerError::ExecutorNotSupported),
        }
    }

    /// Cancel a running job
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

    /// List all active job IDs that were dispatched under the provided lease
    pub async fn find_jobs_by_lease(&self, lease_id: &Uuid) -> Vec<Uuid> {
        self.active_jobs
            .lock()
            .await
            .values()
            .filter_map(|job| (job.lease == *lease_id).then_some(job.id))
            .collect()
    }

    /// List all active job IDs that were dispatched by the provided scheduler
    pub async fn find_jobs_by_scheduler(&self, scheduler: &PeerId) -> Vec<Uuid> {
        self.active_jobs
            .lock()
            .await
            .values()
            .filter_map(|job| (job.scheduler == *scheduler).then_some(job.id))
            .collect()
    }

    /// Get the status of a job
    pub async fn get_status(&self, job_id: &Uuid) -> Result<JobStatus, JobManagerError> {
        // TODO: Status tracking is not implemented yet for type-erased executions.
        // Consider extending `Execution` to expose status signals or storing status separately.
        Err(JobManagerError::TaskNotFound(*job_id))
    }

    /// Clean up completed or failed jobs
    pub async fn cleanup_finished_jobs(&mut self) -> Vec<Uuid> {
        // TODO: Implement cleanup by tracking job completion; currently a no-op.
        Vec::new()
    }

    /// Shutdown the job manager and all executors
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
