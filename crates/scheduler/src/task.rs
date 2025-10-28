use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures_util::{Stream, future::join_all};
use hypha_messages::{JobSpec, JobStatus, api, dispatch_job, job_status};
use hypha_network::request_response::RequestResponseInterfaceExt;
use libp2p::PeerId;
use tokio::{sync::mpsc, task::JoinHandle};
use uuid::Uuid;

use crate::{network::Network, worker::WorkerError};

/// A task represents a task as it is being executed by one or multiple nodes.
/// During its lifetime, it provides a stream of status updates for the task, sent by these nodes.
pub struct Task {
    id: Uuid,
    status_rx: mpsc::Receiver<(PeerId, JobStatus)>,
    status_handler: JoinHandle<()>,
}

impl Task {
    pub async fn try_new(
        network: Network,
        job_spec: JobSpec,
        workers: &[PeerId],
    ) -> Result<Self, WorkerError> {
        let (tx, rx) = mpsc::channel(100);

        let id = Uuid::new_v4();

        let status_handler = tokio::spawn(
            network
                .on::<api::Codec, _>(move |req: &api::Request| {
                    matches!(
                        req,
                        api::Request::JobStatus(
                        job_status::Request { task_id, .. }
                    ) if task_id == &id
                    )
                })
                .into_stream()
                .await
                .map_err(WorkerError::from)?
                .respond_with_concurrent(None, move |request| {
                    let tx = tx.clone();
                    async move {
                        if let (
                            peer_id,
                            api::Request::JobStatus(job_status::Request { status, .. }),
                        ) = request
                        {
                            // Send response
                            let _ = tx.send((peer_id, status.clone())).await;
                            tracing::info!(
                                %peer_id,
                                ?status,
                                "Received status",
                            );
                        }
                        api::Response::JobStatus(job_status::Response {})
                    }
                }),
        );

        let dispatch_futures = workers.iter().map(|peer_id| {
            let network = network.clone();
            let job_spec = job_spec.clone();
            async move {
                match network
                    .request::<api::Codec>(
                        *peer_id,
                        api::Request::DispatchJob(dispatch_job::Request {
                            id,
                            spec: job_spec.clone(),
                        }),
                    )
                    .await
                {
                    Ok(api::Response::DispatchJob(dispatch_job::Response::Dispatched {
                        ..
                    })) => Ok(()),
                    Ok(api::Response::DispatchJob(dispatch_job::Response::Failed)) => Err(
                        WorkerError::DispatchFailed("Worker rejected job".to_string()),
                    ),
                    Ok(_) => Err(WorkerError::DispatchFailed(
                        "Unexpected response type".to_string(),
                    )),
                    Err(e) => Err(WorkerError::DispatchFailed(format!("Network error: {}", e))),
                }
            }
        });

        join_all(dispatch_futures)
            .await
            .into_iter()
            .collect::<Result<(), _>>()?;

        Ok(Self {
            id,
            status_rx: rx,
            status_handler,
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }
}

impl Stream for Task {
    type Item = (PeerId, JobStatus);

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.status_rx.poll_recv(cx)
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        // Ensure that status handling stops if an instance goes out of scope.
        // Task status is tied to the lifetime of a task.
        self.status_handler.abort();
    }
}
