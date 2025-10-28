use std::{
    pin::Pin,
    task::{Context, Poll},
};

use futures_util::{Stream, future::select_all};
use hypha_messages::WorkerSpec;
use libp2p::PeerId;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::{
    allocator::Allocator,
    worker::{Worker, WorkerError},
};

pub enum DiLoCoControllerEvent {
    WorkersAllocated(Vec<PeerId>),
    ParameterServerAllocated(Vec<PeerId>),
    NodeRemoved(Result<(), WorkerError>),
}

pub struct DiLoCoController {
    handler: JoinHandle<()>,
    event_rx: mpsc::Receiver<DiLoCoControllerEvent>,
}

impl DiLoCoController {
    pub fn create<A: Allocator + 'static>(
        allocator: A,
        worker_spec: WorkerSpec,
        parameter_server_spec: WorkerSpec,
        num_workers: usize,
        num_parameter_servers: usize,
    ) -> Self {
        let (tx, rx) = mpsc::channel(100);

        let handler = tokio::spawn(async move {
            let mut worker_nodes = Vec::with_capacity(num_workers);
            let mut parameter_server_nodes = Vec::with_capacity(num_parameter_servers);

            let worker_reconciler = MinWorkersReconciler {
                allocator: &allocator,
                spec: worker_spec,
                min_workers: num_workers,
            };

            let parameter_server_reconciler = MinWorkersReconciler {
                allocator: &allocator,
                spec: parameter_server_spec,
                min_workers: num_parameter_servers,
            };

            // Reconciliation loop: We request new nodes in case existing ones terminate.
            loop {
                if worker_reconciler.reconcile(&mut worker_nodes).await {
                    let _ = tx
                        .send(DiLoCoControllerEvent::WorkersAllocated(
                            worker_nodes.iter().map(|w| w.peer_id()).collect(),
                        ))
                        .await;
                }

                if parameter_server_reconciler
                    .reconcile(&mut parameter_server_nodes)
                    .await
                {
                    let _ = tx
                        .send(DiLoCoControllerEvent::ParameterServerAllocated(
                            parameter_server_nodes.iter().map(|w| w.peer_id()).collect(),
                        ))
                        .await;
                }

                let (res, _, remaining) = select_all(worker_nodes).await;
                worker_nodes = remaining;

                let _ = tx.send(DiLoCoControllerEvent::NodeRemoved(res)).await;
            }
        });

        Self {
            handler,
            event_rx: rx,
        }
    }
}

impl Stream for DiLoCoController {
    type Item = DiLoCoControllerEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.event_rx.poll_recv(cx)
    }
}

impl Drop for DiLoCoController {
    fn drop(&mut self) {
        self.handler.abort();
    }
}

struct MinWorkersReconciler<'a, A: Allocator> {
    allocator: &'a A,
    spec: WorkerSpec,
    min_workers: usize,
}

impl<'a, A: Allocator> MinWorkersReconciler<'a, A> {
    // Request new workers if the current number of workers is less than the minimum required.
    // Returns true if new workers were requested, false otherwise.
    async fn reconcile(&self, workers: &mut Vec<Worker>) -> bool {
        if workers.len() < self.min_workers {
            let diff = self.min_workers - workers.len();
            let allocated_workers = self
                .allocator
                .request(self.spec.clone(), 100.0, None, diff)
                .await
                .unwrap_or_default();

            workers.extend(allocated_workers);

            return true;
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use libp2p::PeerId;
    use mockall::mock;
    use std::{future::ready, time::Duration};
    use uuid::Uuid;

    use crate::allocator::AllocatorError;

    use super::*;

    mock! {
        Allocator {}

        impl Allocator for Allocator {
            async fn request(
                &self,
                spec: WorkerSpec,
                price: f64,
                deadline: Option<Duration>,
                num: usize,
            ) -> Result<Vec<Worker>, AllocatorError>;
        }
    }

    #[tokio::test]
    // Test that calling 'reconcile' with the same or more number as the minimum requested
    // doesn't allocated workers.
    async fn test_reconcile_same_workers() {
        let allocator = MockAllocator::new();
        let spec = WorkerSpec {
            requirements: vec![],
        };

        let reconciler = MinWorkersReconciler {
            allocator: &allocator,
            spec: spec.clone(),
            min_workers: 2,
        };

        let mut workers = vec![
            Worker::new(
                Uuid::new_v4(),
                PeerId::random(),
                spec.clone(),
                100.0,
                tokio::spawn(ready(Ok(()))),
            ),
            Worker::new(
                Uuid::new_v4(),
                PeerId::random(),
                spec.clone(),
                100.0,
                tokio::spawn(ready(Ok(()))),
            ),
        ];

        assert!(!reconciler.reconcile(&mut workers).await);
        assert_eq!(workers.len(), 2);

        workers.push(Worker::new(
            Uuid::new_v4(),
            PeerId::random(),
            spec.clone(),
            100.0,
            tokio::spawn(ready(Ok(()))),
        ));

        assert!(!reconciler.reconcile(&mut workers).await);
        assert_eq!(workers.len(), 3);
    }

    #[tokio::test]
    // Test that calling 'reconcile' with fewer workers than the minimum requested
    // allocates workers.
    async fn test_reconcile_missing_workers() {
        let mut allocator = MockAllocator::new();
        let spec = WorkerSpec {
            requirements: vec![],
        };

        allocator.expect_request().times(1).returning({
            let spec = spec.clone();
            move |_, _, _, _| {
                Ok(vec![
                    Worker::new(
                        Uuid::new_v4(),
                        PeerId::random(),
                        spec.clone(),
                        100.0,
                        tokio::spawn(ready(Ok(()))),
                    ),
                    Worker::new(
                        Uuid::new_v4(),
                        PeerId::random(),
                        spec.clone(),
                        100.0,
                        tokio::spawn(ready(Ok(()))),
                    ),
                ])
            }
        });

        let reconciler = MinWorkersReconciler {
            allocator: &allocator,
            spec,
            min_workers: 2,
        };

        let mut workers = vec![];

        assert!(reconciler.reconcile(&mut workers).await);
        assert_eq!(workers.len(), 2);
    }
}
