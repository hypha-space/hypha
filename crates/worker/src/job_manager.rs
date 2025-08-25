use std::{collections::HashMap, sync::Arc};

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
}

impl JobManager {
    pub fn new(connector: Connector<Network>) -> Self {
        Self {
            active_jobs: Arc::new(Mutex::new(HashMap::new())),
            connector,
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
                let executor = ProcessExecutor::new(self.connector.clone());
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
                let executor = ParameterServerExecutor::new(self.connector.clone());
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
        if let Some(job) = self.active_jobs.lock().await.get(job_id) {
            let _ = job.cancel().await;
            Ok(())
        } else {
            Err(JobManagerError::TaskNotFound(*job_id))
        }
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
        // Cancel all active jobs
        for (_, job) in self.active_jobs.lock().await.iter() {
            let _ = job.cancel().await;
        }
    }
}
