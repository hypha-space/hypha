use std::sync::Arc;

use hypha_messages::progress::{self, Metrics};
use hypha_network::request_response::{RequestResponseError, RequestResponseInterfaceExt};
use libp2p::PeerId;
use thiserror::Error;
use tokio::{
    sync::{
        Mutex, Notify,
        mpsc::{self, Sender, error::SendError},
    },
    task::JoinHandle,
};
use uuid::Uuid;

use crate::{
    network::Network,
    simulation::Simulation,
    statistics::RuntimeStatistic,
    tracker::{
        progress::ProgressTracker,
        worker::{State, WorkerTrackerError},
    },
};

#[derive(Debug, Error)]
pub enum BatchSchedulerError {
    #[error("Disconnected")]
    Disconnected,
    #[error("Unregistering worker")]
    Unregister,
    #[error("Error during scheduling {0}")]
    Scheduling(String),
    #[error("Network error")]
    NetworkError(#[from] RequestResponseError),
    #[error("Send Metrics Error")]
    SendMetricsError(#[from] SendError<(PeerId, Metrics)>),
    #[error("Tracker error")]
    TrackerError(#[from] WorkerTrackerError),
}

/// The Batch Scheduler uses internally the following states
/// for scheduling
/// for scheduling
/// ```mermaid
/// flowchart TD
/// A[Training] --> |Worker: Status| B{Project}
/// B --> |Not done| A
/// B -->|Done| C[UpdateScheduled]
/// C -->|Worker: Update| E[Updating]
/// E -->|PS: Updated| F[Update Received]
/// F --> A
/// ```
async fn schedule<T, S>(
    tx: Sender<(PeerId, Metrics)>,
    tracker: Arc<Mutex<ProgressTracker<T>>>,
    request: (PeerId, progress::Request),
    notifier: Arc<Notify>,
) -> Result<progress::Response, BatchSchedulerError>
where
    T: RuntimeStatistic + 'static,
    S: Simulation + Send + Sync + 'static,
{
    let (
        peer_id,
        progress::Request {
            progress: update, ..
        },
    ) = request;
    tracing::info!(
        %peer_id,
        ?update,
        "Received update",
    );
    match update {
        progress::Progress::Metrics(metrics) => {
            tx.send((peer_id, metrics))
                .await
                .map_err(BatchSchedulerError::from)?;
            Ok(progress::Response::Ok {})
        }
        progress::Progress::Status(progress::Status { batch_size }) => {
            let mut tracker = tracker.lock().await;
            tracker.update(&peer_id, batch_size)?;
            match tracker.worker_tracker.worker_state(&peer_id) {
                State::Training => {
                    // NOTE: time_cap is u64 (time), update_cap is u32 (count)
                    let time_cap: u64 = 10000;
                    let update_cap: u32 = 3;
                    let (time, cnt, projection, capped) = S::project(
                        tracker.worker_tracker.last_updates(),
                        tracker.worker_tracker.batch_sizes(),
                        tracker.worker_tracker.estimates(),
                        tracker.count(),
                        time_cap,
                        update_cap,
                    );
                    let peer_position = tracker.worker_tracker.worker_position(&peer_id)?;
                    tracing::debug!(time = %time, count = %cnt, "Finish simulation");
                    tracing::info!(
                        "Simulation with projection {:?} and {:?} of {:?}",
                        projection,
                        cnt,
                        tracker.count()
                    );
                    if cnt == 0 && !capped {
                        tracker
                            .worker_tracker
                            .update_worker_state(&peer_id, State::UpdateScheduled);
                        Ok(progress::Response::ScheduleUpdate {
                            counter: projection[peer_position],
                        })
                    } else {
                        Ok(progress::Response::Continue {})
                    }
                }
                State::UpdateScheduled => Ok(progress::Response::Continue {}),
                _ => Err(BatchSchedulerError::Scheduling("Wrong state".into())),
            }
        }
        progress::Progress::Update => {
            // ToDo. Track times for update scheduled and force update when a worker
            // isn't responsive
            tracker
                .lock()
                .await
                .worker_tracker
                .update_worker_state(&peer_id, State::Updating);
            Ok(progress::Response::Ok {})
        }
        progress::Progress::Updated => {
            let mut tracker = tracker.lock().await;
            tracker.next_round();
            if tracker.training_finished() {
                Ok(progress::Response::Done {})
            } else {
                Ok(progress::Response::Ok {})
            }
        }
        progress::Progress::UpdateReceived => {
            let mut tracker = tracker.lock().await;
            if tracker.training_finished() {
                tracker
                    .worker_tracker
                    .update_worker_state(&peer_id, State::Done);
                if tracker
                    .worker_tracker
                    .states()
                    .iter()
                    .all(|state| state == &State::Done)
                {
                    notifier.notify_one();
                }
                Ok(progress::Response::Done {})
            } else {
                tracker
                    .worker_tracker
                    .update_worker_state(&peer_id, State::Training);
                Ok(progress::Response::Continue {})
            }
        }
    }
}

pub struct BatchScheduler {}

impl BatchScheduler {
    pub async fn run<T, S>(
        network: Network,
        tracker: Arc<Mutex<ProgressTracker<T>>>,
        id: Uuid,
    ) -> Result<(mpsc::Receiver<(PeerId, Metrics)>, JoinHandle<()>), BatchSchedulerError>
    where
        T: RuntimeStatistic + 'static,
        S: Simulation + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::channel(100);
        let shutdown_notify = Arc::new(Notify::new());
        let stream_handle = tokio::spawn({
            let shutdown_notify = shutdown_notify.clone();
            network
                .on::<progress::Codec, _>(move |req: &progress::Request| {
                    matches!(
                        req,
                        progress::Request{job_id, ..}
                    if &id == job_id
                    )
                })
                .into_stream()
                .await
                .map_err(BatchSchedulerError::from)?
                .respond_with_concurrent(None, move |request| {
                    let tx = tx.clone();
                    let tracker = tracker.clone();
                    let shutdown_notify = shutdown_notify.clone();
                    async move {
                        match schedule::<T, S>(tx, tracker, request, shutdown_notify).await {
                            Ok(response) => response,
                            Err(e) => {
                                tracing::warn!(error = ?e, "Error handling request");
                                progress::Response::Error {}
                            }
                        }
                    }
                })
        });

        let handle = tokio::spawn(async move {
            let shutdown_future = shutdown_notify.notified();
            tokio::select! {
                _ = stream_handle => {
                    tracing::info!("Stream handler finished.");
                },
                _ = shutdown_future => {
                    tracing::info!("Job is completed.");
                }
            }
        });
        Ok((rx, handle))
    }
}

#[cfg(test)]
mod batch_scheduler_tests {
    use std::{collections::HashMap, sync::Arc};

    use hypha_messages::progress::{
        self, Metrics,
        Progress::{Status, Update, UpdateReceived, Updated},
        Response,
    };
    use libp2p::PeerId;
    use tokio::{
        sync::{
            Mutex, Notify,
            mpsc::{Sender, channel},
        },
        time::Duration,
    };
    use uuid::Uuid;

    use super::schedule;
    use crate::{
        simulation::BasicSimulation,
        statistics::RunningMean,
        tracker::{progress::ProgressTracker, worker::State},
    };

    #[tokio::test]
    async fn handle_metrics() {
        let notify = Arc::new(Notify::new());
        let tracker = Arc::new(Mutex::new(ProgressTracker::<RunningMean>::new(
            PeerId::random(),
            1024,
            2,
        )));
        let (tx, mut rx) = channel(3);
        let peer_id = PeerId::random();
        let metrics = progress::Metrics {
            round: 1,
            metrics: HashMap::from([("Loss".into(), 1.2), ("Acc".into(), 1.4)]),
        };
        let request = (
            peer_id,
            progress::Request {
                job_id: Uuid::new_v4(),
                progress: progress::Progress::Metrics(metrics.clone()),
            },
        );

        let result =
            schedule::<RunningMean, BasicSimulation>(tx, tracker, request, notify.clone()).await;
        assert!(result.is_ok());
        assert_eq!(
            serde_json::to_string(&result.unwrap()).unwrap(),
            serde_json::to_string(&progress::Response::Ok).unwrap()
        );
        assert!(rx.len() == 1);
        let (rec_peer_id, rec_metrics) = rx.recv().await.unwrap();
        assert_eq!(rec_peer_id, peer_id);
        assert_eq!(
            serde_json::to_string(&rec_metrics).unwrap(),
            serde_json::to_string(&metrics).unwrap()
        );
    }

    async fn advance_and_assert(
        tx: Sender<(PeerId, Metrics)>,
        tracker: Arc<Mutex<ProgressTracker<RunningMean>>>,
        job_id: Uuid,
        steps: Vec<Step>,
    ) {
        let notify = Arc::new(Notify::new());
        for (i, step) in steps.iter().enumerate() {
            tokio::time::advance(Duration::from_millis(step.time)).await;
            tokio::time::resume();
            assert_eq!(
                serde_json::to_string(
                    &schedule::<RunningMean, BasicSimulation>(
                        tx.clone(),
                        tracker.clone(),
                        (
                            step.worker_id,
                            progress::Request {
                                job_id,
                                progress: step.request.clone()
                            }
                        ),
                        notify.clone()
                    )
                    .await
                    .unwrap()
                )
                .unwrap(),
                serde_json::to_string(&step.response).unwrap(),
                "Failed in step {:?}",
                i
            );
            tokio::time::pause();
        }
    }

    struct Step {
        pub worker_id: PeerId,
        pub request: progress::Progress,
        pub response: progress::Response,
        pub time: u64,
    }

    impl Step {
        pub fn new(
            worker_id: PeerId,
            request: progress::Progress,
            response: progress::Response,
            time: u64,
        ) -> Self {
            Self {
                worker_id,
                request,
                response,
                time,
            }
        }
    }

    #[tokio::test]
    async fn handle_simple_two_rounds() {
        let ps = PeerId::random();
        let tracker = Arc::new(Mutex::new(ProgressTracker::<RunningMean>::new(ps, 800, 2)));
        tokio::time::pause();
        let w1 = PeerId::random();
        tracker.lock().await.worker_tracker.add_worker(w1, 150);
        let w2 = PeerId::random();
        tracker.lock().await.worker_tracker.add_worker(w2, 100);
        let (tx, _) = channel(1);
        let w3 = PeerId::random();
        tracker.lock().await.worker_tracker.add_worker(w3, 50);
        let job_id = Uuid::new_v4();

        // Start : Updates received [0, 0, 0], update times [0, 0, 0], cnt 800, t=0,
        // W3 Status: Updates received [0, 0, 1], times [0, 0, 500], cnt 750, t=500
        // W2 Status: Updates received [0, 1, 1], times [0, 800, 500], cnt 650, t=800
        // W1 Status: Updates received [1, 1, 1], times [950, 800, 500], cnt 500, t=950, Schedule Update W1
        // W3 Status: Updates received [1, 1, 2], times [950, 800, 1000], cnt 450, t=1000
        // W3 Status: Updates received [1, 1, 3], times [950, 800, 1500], cnt 400, t=1500
        // W2 Status: Updates received [1, 2, 3], times [950, 1600, 1500], cnt 300, t=1600, Schedule Update W2
        // W1 Status: Updates received [2, 2, 3], times [1900, 1600, 1500], cnt 150, t=1900
        // W3 Status: Updates received [2, 2, 4], times [1900, 1600, 2000], cnt 100, t=2000, Schedule Update W3
        // W2 Status: Updates received [2, 3, 4], times [1900, 2400, 2000], cnt 0, t=2400
        // W1, W2, W3 send Updating
        // PS send Updated
        // W1, W2, W3 receive Update
        // Start new round

        let steps = vec![
            Step::new(
                w3,
                Status(progress::Status { batch_size: 50 }),
                Response::Continue,
                500,
            ),
            Step::new(
                w2,
                Status(progress::Status { batch_size: 100 }),
                Response::Continue,
                300,
            ),
            Step::new(
                w1,
                Status(progress::Status { batch_size: 150 }),
                Response::Continue,
                150,
            ),
            Step::new(
                w3,
                Status(progress::Status { batch_size: 50 }),
                Response::ScheduleUpdate { counter: 2 },
                50,
            ),
            Step::new(
                w3,
                Status(progress::Status { batch_size: 50 }),
                Response::Continue,
                500,
            ),
            Step::new(
                w2,
                Status(progress::Status { batch_size: 100 }),
                Response::ScheduleUpdate { counter: 1 },
                100,
            ),
            Step::new(
                w1,
                Status(progress::Status { batch_size: 150 }),
                Response::ScheduleUpdate { counter: 0 },
                300,
            ),
            Step::new(
                w3,
                Status(progress::Status { batch_size: 50 }),
                Response::Continue,
                100,
            ),
            Step::new(
                w2,
                Status(progress::Status { batch_size: 100 }),
                Response::Continue,
                100,
            ),
            Step::new(w1, Update {}, Response::Ok, 0),
            Step::new(w2, Update {}, Response::Ok, 0),
            Step::new(w3, Update {}, Response::Ok, 0),
            Step::new(ps, Updated {}, Response::Ok, 0),
            Step::new(w1, UpdateReceived {}, Response::Continue, 0),
            Step::new(w2, UpdateReceived {}, Response::Continue, 0),
            Step::new(w3, UpdateReceived {}, Response::Continue, 0),
        ];

        advance_and_assert(tx.clone(), tracker.clone(), job_id, steps).await;

        assert_eq!(
            tracker.lock().await.worker_tracker.states(),
            vec![State::Training; 3]
        );
        assert_eq!(tracker.lock().await.count(), 800);
        assert_eq!(tracker.lock().await.round(), 1);
    }

    #[tokio::test]
    async fn single_worker_responding() {
        let ps = PeerId::random();
        let tracker = Arc::new(Mutex::new(ProgressTracker::<RunningMean>::new(ps, 200, 2)));
        tokio::time::pause();
        let w1 = PeerId::random();
        tracker.lock().await.worker_tracker.add_worker(w1, 150);
        let w2 = PeerId::random();
        tracker.lock().await.worker_tracker.add_worker(w2, 100);
        let (tx, _) = channel(1);
        let job_id = Uuid::new_v4();

        let steps = vec![
            Step::new(
                w1,
                Status(progress::Status { batch_size: 100 }),
                Response::ScheduleUpdate { counter: 1 },
                100,
            ),
            Step::new(
                w1,
                Status(progress::Status { batch_size: 100 }),
                Response::Continue {},
                100,
            ),
        ];

        advance_and_assert(tx.clone(), tracker.clone(), job_id, steps).await;

        assert_eq!(
            tracker.lock().await.worker_tracker.states(),
            vec![State::UpdateScheduled, State::Training]
        );
    }
}
