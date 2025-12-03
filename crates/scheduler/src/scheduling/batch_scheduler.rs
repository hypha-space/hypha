use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use hypha_messages::{
    Reference, SelectionStrategy,
    action::{
        self, AggregateAction, AggregateError, AggregateStatus, ExecutorAction, ExecutorStatus,
        TrainAction, TrainError, TrainStatus,
    },
    progress::Metrics,
};
use hypha_network::request_response::{RequestResponseError, RequestResponseInterfaceExt};
use hypha_resources::Resources;
use libp2p::PeerId;
use thiserror::Error;
use tokio::{
    sync::{
        Mutex,
        mpsc::{self, Sender, error::SendError},
    },
    task::JoinHandle,
};
use uuid::Uuid;

use crate::{
    network::Network,
    pool::{PoolHandle, PoolStatisticsHandle},
    simulation::Simulation,
    statistics::RuntimeStatistic,
};

// NOTE: Tracks per-round update signals from workers so the scheduler can
// decide when to instruct the parameter server to aggregate.
#[derive(Default)]
struct RoundState {
    sent_updates: HashSet<PeerId>,
    first_update_at: Option<Instant>,
    min_quorum: usize,
    grace: Duration,
    round: u32,
}

#[derive(Default)]
struct TrainingState {
    update_target: u32,
    counter: u32,
}

impl TrainingState {
    fn new(update_target: u32) -> Self {
        Self {
            update_target,
            counter: update_target,
        }
    }

    fn record_batch(&mut self, batch_size: u32) {
        self.counter = self.counter.saturating_sub(batch_size);
    }

    fn samples_remaining(&self) -> u32 {
        self.counter
    }

    fn reset_round(&mut self) {
        self.counter = self.update_target;
    }
}

// NOTE: time_cap is u64 (time), update_cap is u32 (count)
const SIM_TIME_CAP_MS: u64 = 10_000;
const SIM_UPDATE_CAP: u32 = 3;

type BatchSizer = Arc<dyn Fn(&Resources) -> u32 + Send + Sync>;

#[derive(Debug, Error)]
pub enum BatchSchedulerError {
    #[error("Disconnected")]
    Disconnected,
    #[error("Unregistering worker")]
    Unregister,
    #[error("Error during scheduling {0}")]
    Scheduling(String),
    #[error("Network error: {0}")]
    NetworkError(#[from] RequestResponseError),
    #[error("Send Metrics Error: {0}")]
    SendMetricsError(#[from] SendError<(PeerId, Metrics)>),
}

/// Handle action protocol requests and respond with next steps.
async fn schedule<T, S>(
    tx: Sender<(PeerId, Metrics)>,
    worker_pool: PoolStatisticsHandle<T>,
    parameter_pool: PoolHandle,
    round_state: Arc<Mutex<RoundState>>,
    training_state: Arc<Mutex<TrainingState>>,
    batch_sizer: BatchSizer,
    start: std::time::Instant,
    request: (PeerId, action::ActionRequest),
) -> Result<action::ActionResponse, BatchSchedulerError>
where
    T: RuntimeStatistic + 'static,
    S: Simulation + Send + Sync + 'static,
{
    let _ = std::marker::PhantomData::<S>;
    let (peer_id, action::ActionRequest { job_id, status }) = request;
    tracing::debug!(%peer_id, ?status, %job_id, "Received action request");

    let now = SystemTime::now();

    let since_start = start.elapsed().as_millis() as u64;
    // NOTE: We rely on Pool::members() being oldest-first ordered by join time.
    let parameter_servers: Vec<PeerId> =
        parameter_pool.members().iter().map(|w| w.peer_id).collect();
    let primary_ps = parameter_servers.first().copied();

    let response = match status {
        ExecutorStatus::Train(train) => match train {
            TrainStatus::Idle => ExecutorAction::Train(TrainAction::ExecuteBatch),
            TrainStatus::BatchCompleted { batch_size } => {
                worker_pool.update(&peer_id, since_start);

                let snapshot = worker_pool.statistics();
                let mut training = training_state.lock().await;
                training.record_batch(batch_size);
                let samples_remaining = training.samples_remaining();
                drop(training);

                if parameter_servers.is_empty() {
                    return Ok(action::ActionResponse {
                        job_id,
                        next: ExecutorAction::Train(TrainAction::Idle {
                            timeout: now + Duration::from_secs(1),
                        }),
                    });
                }

                let stats: Vec<u64> = snapshot.iter().map(|w| w.statistic.unwrap_or(0)).collect();
                let progress: Vec<u64> = snapshot
                    .iter()
                    .map(|w| w.last_updated.unwrap_or(0))
                    .collect();
                let batch_sizes: Vec<u32> = snapshot
                    .iter()
                    .map(|w| (batch_sizer)(&w.resources))
                    .collect();

                let should_update = if samples_remaining == 0 {
                    true
                } else if !snapshot.is_empty()
                    && batch_sizes.iter().all(|&b| b > 0)
                    && stats.iter().all(|&s| s > 0 && s < u64::MAX)
                {
                    let (time, cnt, projection, capped) = S::project(
                        &progress,
                        &batch_sizes,
                        stats,
                        samples_remaining,
                        SIM_TIME_CAP_MS,
                        SIM_UPDATE_CAP,
                    );
                    let peer_position = snapshot
                        .iter()
                        .position(|w| w.peer_id == peer_id)
                        .unwrap_or(0);

                    tracing::debug!(
                        time = %time,
                        count = %cnt,
                        peer = %peer_id,
                        "Simulation with projection {:?} and {:?}",
                        projection,
                        samples_remaining
                    );
                    cnt == 0
                        && !capped
                        && peer_position < projection.len()
                        && projection[peer_position] == 0
                } else {
                    false
                };

                if should_update {
                    ExecutorAction::Train(TrainAction::SendUpdate {
                        target: Reference::Peers {
                            // Selecting a single PS to avoid that workers send updates to multiple PS
                            peers: vec![parameter_servers[0]],
                            strategy: SelectionStrategy::One,
                            resource: None,
                        },
                        // TODO: We need a way to properly determine a good sent timeout
                        timeout: now + Duration::from_secs(30),
                    })
                } else {
                    ExecutorAction::Train(TrainAction::ExecuteBatch)
                }
            }
            TrainStatus::SentUpdate => {
                // NOTE: Track workers that have sent their update for the current round.
                let mut state = round_state.lock().await;
                state.sent_updates.insert(peer_id);
                if state.first_update_at.is_none() {
                    state.first_update_at = Some(Instant::now());
                }
                let total_workers = worker_pool.statistics().len();
                let sent = state.sent_updates.len();
                let elapsed_ms = state
                    .first_update_at
                    .map(|t| t.elapsed().as_millis() as u64)
                    .unwrap_or(0);
                tracing::info!(
                    %peer_id,
                    round = state.round,
                    sent,
                    total = total_workers,
                    min_quorum = state.min_quorum,
                    grace_ms = state.grace.as_millis() as u64,
                    since_first_ms = elapsed_ms,
                    "Worker reported SentUpdate; recorded for round"
                );
                if parameter_servers.is_empty() {
                    ExecutorAction::Train(TrainAction::Idle {
                        timeout: now + Duration::from_secs(1),
                    })
                } else {
                    ExecutorAction::Train(TrainAction::ApplyUpdate {
                        source: Reference::Peers {
                            peers: parameter_servers,
                            strategy: SelectionStrategy::All,
                            resource: None,
                        },
                        timeout: now + Duration::from_secs(30),
                    })
                }
            }
            TrainStatus::AppliedUpdate { round, metrics } => {
                tx.send((peer_id, Metrics { round, metrics }))
                    .await
                    .map_err(BatchSchedulerError::from)?;

                ExecutorAction::Train(TrainAction::ExecuteBatch)
            }
            TrainStatus::Error(TrainError::Connection { message }) => {
                tracing::warn!(%peer_id, message = %message, "Worker reported connection error");
                ExecutorAction::Train(TrainAction::Idle {
                    timeout: now + Duration::from_secs(1),
                })
            }
            TrainStatus::Error(TrainError::Other { message }) => {
                tracing::warn!(%peer_id, message = %message, "Worker reported error");
                ExecutorAction::Train(TrainAction::Terminate)
            }
            TrainStatus::Terminated => ExecutorAction::Train(TrainAction::Terminate),
        },
        ExecutorStatus::Aggregate(state) => match state {
            AggregateStatus::Idle => {
                // Only the primary PS is allowed to aggregate.
                if Some(peer_id) != primary_ps {
                    if let Some(primary) = primary_ps {
                        tracing::debug!(
                            %peer_id,
                            primary_ps = %primary,
                            "Non-primary PS polling; returning Idle"
                        );
                    }
                    ExecutorAction::Aggregate(AggregateAction::Idle {
                        timeout: now + Duration::from_secs(5),
                    })
                } else {
                    let workers: Vec<_> = worker_pool
                        .statistics()
                        .into_iter()
                        .map(|w| w.peer_id)
                        .collect();

                    if workers.is_empty() {
                        ExecutorAction::Aggregate(AggregateAction::Idle {
                            timeout: now + Duration::from_secs(1),
                        })
                    } else {
                        // Start aggregation when either all workers have sent updates,
                        // or when a quorum (min workers) have sent updates and the
                        // grace period has elapsed since the first update in this round.
                        let state = round_state.lock().await;
                        let all_sent = workers.iter().all(|w| state.sent_updates.contains(w));
                        let effective_quorum = state.min_quorum.min(workers.len());
                        let quorum_met = state.sent_updates.len() >= effective_quorum;
                        let timebox_elapsed = state
                            .first_update_at
                            .map(|t| t.elapsed() >= state.grace)
                            .unwrap_or(false);
                        let ready = all_sent || (quorum_met && timebox_elapsed);
                        let reason = if all_sent {
                            "all_sent"
                        } else if quorum_met && timebox_elapsed {
                            "quorum_and_grace"
                        } else if quorum_met {
                            "quorum_met_waiting_grace"
                        } else {
                            "waiting_quorum"
                        };
                        tracing::info!(
                            round = state.round,
                            workers = workers.len(),
                            sent = state.sent_updates.len(),
                            min_quorum = state.min_quorum,
                            effective_quorum,
                            grace_ms = state.grace.as_millis() as u64,
                            since_first_ms = state
                                .first_update_at
                                .map(|t| t.elapsed().as_millis() as u64)
                                .unwrap_or(0),
                            ready,
                            reason,
                            "Aggregation readiness evaluation"
                        );

                        if ready {
                            tracing::info!(round = state.round, "Trigger AggregateUpdates");
                            ExecutorAction::Aggregate(AggregateAction::AggregateUpdates {
                                source: Reference::Peers {
                                    peers: workers,
                                    strategy: SelectionStrategy::All,
                                    resource: None,
                                },
                            })
                        } else {
                            ExecutorAction::Aggregate(AggregateAction::Idle {
                                timeout: now + Duration::from_millis(500),
                            })
                        }
                    }
                }
            }
            AggregateStatus::AggregatedUpdates { .. } => {
                // Only allow the primary PS to proceed to broadcast.
                if Some(peer_id) != primary_ps {
                    ExecutorAction::Aggregate(AggregateAction::Idle {
                        timeout: now + Duration::from_secs(5),
                    })
                } else {
                    let workers: Vec<_> = worker_pool
                        .statistics()
                        .into_iter()
                        .map(|w| w.peer_id)
                        .collect();

                    if workers.is_empty() {
                        ExecutorAction::Aggregate(AggregateAction::Idle {
                            timeout: now + Duration::from_secs(1),
                        })
                    } else {
                        // Log that we are moving to broadcast for this round.
                        let state = round_state.lock().await;
                        tracing::info!(round = state.round, "Trigger BroadcastUpdate");
                        ExecutorAction::Aggregate(AggregateAction::BroadcastUpdate {
                            target: Reference::Peers {
                                peers: workers,
                                strategy: SelectionStrategy::All,
                                resource: None,
                            },
                        })
                    }
                }
            }
            AggregateStatus::BroadcastedUpdate { metrics } => {
                if let Some(metrics) = metrics {
                    tx.send((peer_id, Metrics { round: 0, metrics }))
                        .await
                        .map_err(BatchSchedulerError::from)?;
                }
                // Reset round state after completing a broadcast on the primary PS.
                if Some(peer_id) == primary_ps {
                    let mut state = round_state.lock().await;
                    tracing::info!(round = state.round, "Broadcast completed; advancing round");
                    state.sent_updates.clear();
                    state.first_update_at = None;
                    state.round = state.round.saturating_add(1);
                    let next_round = state.round;
                    drop(state);

                    let mut training = training_state.lock().await;
                    training.reset_round();
                    tracing::info!(
                        round = next_round,
                        "Next round started; training state reset"
                    );
                }
                ExecutorAction::Aggregate(AggregateAction::Idle {
                    timeout: now + Duration::from_secs(1),
                })
            }
            AggregateStatus::Error(AggregateError::Connection { message }) => {
                tracing::warn!(%peer_id, message = %message, "Aggregator reported connection error");
                ExecutorAction::Aggregate(AggregateAction::Idle {
                    timeout: now + Duration::from_secs(1),
                })
            }
            AggregateStatus::Error(AggregateError::Other { message }) => {
                tracing::warn!(%peer_id, message = %message, "Aggregator reported error");
                ExecutorAction::Aggregate(AggregateAction::Terminate)
            }

            AggregateStatus::Terminated => ExecutorAction::Aggregate(AggregateAction::Terminate),
        },
    };

    tracing::debug!(%peer_id, %job_id, response = ?response, "Sending action response");

    Ok(action::ActionResponse {
        job_id,
        next: response,
    })
}

pub struct BatchScheduler {}

impl BatchScheduler {
    pub async fn run<T, S>(
        network: Network,
        worker_pool: PoolStatisticsHandle<T>,
        parameter_pool: PoolHandle,
        id: Uuid,
        min_quorum: usize,
        grace: Duration,
        samples_between_updates: u32,
        batch_sizer: BatchSizer,
    ) -> Result<(mpsc::Receiver<(PeerId, Metrics)>, JoinHandle<()>), BatchSchedulerError>
    where
        T: RuntimeStatistic + 'static,
        S: Simulation + Send + Sync + 'static,
    {
        let _ = std::marker::PhantomData::<S>;
        let (tx, rx) = mpsc::channel(100);
        let start = std::time::Instant::now();
        let stream_handle = tokio::spawn({
            let worker_pool = worker_pool.clone();
            let parameter_pool = parameter_pool.clone();
            let batch_sizer = batch_sizer.clone();
            // NOTE: Track per-round SentUpdate signals to decide when to trigger aggregation.
            let round_state = Arc::new(Mutex::new(RoundState {
                sent_updates: HashSet::new(),
                first_update_at: None,
                min_quorum,
                grace,
                round: 0,
            }));
            let training_state = Arc::new(Mutex::new(TrainingState::new(samples_between_updates)));
            network
                .on::<action::Codec, _>(move |req: &action::ActionRequest| {
                    matches!(
                        req,
                        action::ActionRequest{job_id, ..}
                    if &id == job_id
                    )
                })
                .into_stream()
                .await
                .map_err(BatchSchedulerError::from)?
                .respond_with_concurrent(None, move |request| {
                    let tx = tx.clone();
                    let worker_pool = worker_pool.clone();
                    let parameter_pool = parameter_pool.clone();
                    let round_state = round_state.clone();
                    let training_state = training_state.clone();
                    let batch_sizer = batch_sizer.clone();
                    async move {
                        match schedule::<T, S>(
                            tx,
                            worker_pool,
                            parameter_pool,
                            round_state,
                            training_state,
                            batch_sizer,
                            start,
                            request,
                        )
                        .await
                        {
                            Ok(response) => response,
                            Err(e) => {
                                tracing::warn!(error = ?e, "Error handling request");
                                action::ActionResponse {
                                    job_id: id,
                                    next: ExecutorAction::Train(TrainAction::Terminate),
                                }
                            }
                        }
                    }
                })
        });

        let handle = tokio::spawn(async move {
            if let Err(e) = stream_handle.await {
                tracing::warn!(error = ?e, "Stream handler finished with error");
            }
        });
        Ok((rx, handle))
    }
}

#[cfg(test)]
mod batch_scheduler_tests {
    use futures_util::StreamExt;
    use hypha_messages::{
        Reference, SelectionStrategy,
        action::{
            ActionRequest, AggregateStatus, ExecutorAction, ExecutorStatus, TrainAction,
            TrainStatus,
        },
        progress::Metrics,
    };
    use hypha_resources::Resources;
    use libp2p::PeerId;
    use std::collections::HashMap;
    use std::time::SystemTime;
    use tokio::time::Duration;
    use uuid::Uuid;

    use super::{RoundState, TrainingState, schedule};
    use crate::{
        allocator::{Allocator, AllocatorError},
        pool::{Pool, PoolConfig, PoolWithStatistics},
        scheduler_config::PriceRange,
        simulation::BasicSimulation,
        statistics::{RunningMean, RuntimeStatistic},
        worker::{TestWorkerBuilder, Worker},
    };

    struct NoopAllocator;

    impl Allocator for NoopAllocator {
        fn request(
            &self,
            _spec: hypha_messages::WorkerSpec,
            _price: PriceRange,
            _deadline: Option<std::time::Duration>,
            _num: usize,
        ) -> impl std::future::Future<Output = Result<Vec<Worker>, AllocatorError>> + Send {
            async { Ok(Vec::new()) }
        }
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct TestStat {
        last_updated: u64,
        value: u64,
    }

    impl RuntimeStatistic for TestStat {
        fn update(&mut self, time: u64) {
            let delta = time.saturating_sub(self.last_updated);
            self.last_updated = time;
            self.value = delta.max(1);
        }

        fn value(&self) -> u64 {
            self.value
        }
    }

    struct StaticAllocator {
        responses: tokio::sync::Mutex<Vec<Vec<Worker>>>,
    }

    impl StaticAllocator {
        fn new(responses: Vec<Vec<Worker>>) -> Self {
            Self {
                responses: tokio::sync::Mutex::new(responses),
            }
        }
    }

    impl Allocator for StaticAllocator {
        fn request(
            &self,
            _spec: hypha_messages::WorkerSpec,
            _price: PriceRange,
            _deadline: Option<std::time::Duration>,
            num: usize,
        ) -> impl std::future::Future<Output = Result<Vec<Worker>, AllocatorError>> + Send {
            async move {
                let mut responses = self.responses.lock().await;
                if let Some(mut batch) = responses.pop() {
                    let workers: Vec<Worker> = batch.drain(..).take(num).collect();
                    if workers.is_empty() {
                        Err(AllocatorError::NoOffersReceived)
                    } else {
                        Ok(workers)
                    }
                } else {
                    Err(AllocatorError::NoOffersReceived)
                }
            }
        }
    }

    #[tokio::test]
    async fn train_idle_executes_batch() {
        let pool = Pool::new(
            NoopAllocator,
            PoolConfig {
                name: "test".into(),
                spec: hypha_messages::WorkerSpec {
                    resources: Resources::default(),
                    executor: vec![],
                },
                price: PriceRange::default(),
                min: 0,
                target: 0,
                grace: Duration::from_secs(1),
            },
        );
        let worker_pool = PoolWithStatistics::<RunningMean>::new(pool);
        let worker_handle = worker_pool.handle();
        let ps_pool = Pool::new(
            NoopAllocator,
            PoolConfig {
                name: "ps".into(),
                spec: hypha_messages::WorkerSpec {
                    resources: Resources::default(),
                    executor: vec![],
                },
                price: PriceRange::default(),
                min: 0,
                target: 0,
                grace: Duration::from_secs(1),
            },
        );
        let parameter_pool = ps_pool.handle();

        let (tx, _rx) = tokio::sync::mpsc::channel::<(PeerId, Metrics)>(1);
        let round = std::sync::Arc::new(tokio::sync::Mutex::new(RoundState::default()));
        let training_state = std::sync::Arc::new(tokio::sync::Mutex::new(TrainingState::new(10)));
        let batch_sizer = std::sync::Arc::new(|_: &Resources| 1u32);
        let resp = schedule::<RunningMean, BasicSimulation>(
            tx,
            worker_handle,
            parameter_pool,
            round,
            training_state,
            batch_sizer,
            std::time::Instant::now(),
            (
                PeerId::random(),
                ActionRequest {
                    job_id: Uuid::new_v4(),
                    status: ExecutorStatus::Train(TrainStatus::Idle),
                },
            ),
        )
        .await
        .unwrap();

        match resp.next {
            hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch) => {}
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[tokio::test]
    async fn train_waits_without_parameter_server() {
        let pool = Pool::new(
            NoopAllocator,
            PoolConfig {
                name: "test".into(),
                spec: hypha_messages::WorkerSpec {
                    resources: Resources::default(),
                    executor: vec![],
                },
                price: PriceRange::default(),
                min: 0,
                target: 0,
                grace: Duration::from_secs(1),
            },
        );
        let worker_pool = PoolWithStatistics::<RunningMean>::new(pool);
        let worker_handle = worker_pool.handle();
        let ps_pool = Pool::new(
            NoopAllocator,
            PoolConfig {
                name: "ps".into(),
                spec: hypha_messages::WorkerSpec {
                    resources: Resources::default(),
                    executor: vec![],
                },
                price: PriceRange::default(),
                min: 0,
                target: 0,
                grace: Duration::from_secs(1),
            },
        );
        let parameter_pool = ps_pool.handle();

        let (tx, _rx) = tokio::sync::mpsc::channel::<(PeerId, Metrics)>(1);
        let round = std::sync::Arc::new(tokio::sync::Mutex::new(RoundState::default()));
        let training_state = std::sync::Arc::new(tokio::sync::Mutex::new(TrainingState::new(10)));
        let batch_sizer = std::sync::Arc::new(|_: &Resources| 1u32);
        let resp = schedule::<RunningMean, BasicSimulation>(
            tx,
            worker_handle,
            parameter_pool,
            round,
            training_state,
            batch_sizer,
            std::time::Instant::now(),
            (
                PeerId::random(),
                ActionRequest {
                    job_id: Uuid::new_v4(),
                    status: ExecutorStatus::Train(TrainStatus::BatchCompleted { batch_size: 4 }),
                },
            ),
        )
        .await
        .unwrap();

        match resp.next {
            hypha_messages::action::ExecutorAction::Train(TrainAction::Idle { .. }) => {}
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_simple_two_rounds_simulation_drives_updates() {
        struct Step {
            peer: PeerId,
            status: ExecutorStatus,
            expect: ExecutorAction,
            elapsed_ms: u64,
        }

        impl Step {
            fn new(
                peer: PeerId,
                status: ExecutorStatus,
                expect: ExecutorAction,
                elapsed_ms: u64,
            ) -> Self {
                Self {
                    peer,
                    status,
                    expect,
                    elapsed_ms,
                }
            }
        }

        let w1_id = PeerId::random();
        let w2_id = PeerId::random();
        let w3_id = PeerId::random();
        let ps_id = PeerId::random();

        let worker_allocator = StaticAllocator::new(vec![vec![
            TestWorkerBuilder::new()
                .with_peer_id(w1_id)
                .with_resources(Resources::default().with_gpu(150.0))
                .build(),
            TestWorkerBuilder::new()
                .with_peer_id(w2_id)
                .with_resources(Resources::default().with_gpu(100.0))
                .build(),
            TestWorkerBuilder::new()
                .with_peer_id(w3_id)
                .with_resources(Resources::default().with_gpu(50.0))
                .build(),
        ]]);

        let mut worker_pool_stream = PoolWithStatistics::<TestStat>::new(Pool::new(
            worker_allocator,
            PoolConfig {
                name: "workers".into(),
                spec: hypha_messages::WorkerSpec {
                    resources: Resources::default(),
                    executor: vec![],
                },
                price: PriceRange::default(),
                min: 0,
                target: 3,
                grace: Duration::from_secs(1),
            },
        ));
        let worker_handle = worker_pool_stream.handle();

        let parameter_allocator = StaticAllocator::new(vec![vec![
            TestWorkerBuilder::new().with_peer_id(ps_id).build(),
        ]]);

        let mut parameter_pool_stream = Pool::new(
            parameter_allocator,
            PoolConfig {
                name: "ps".into(),
                spec: hypha_messages::WorkerSpec {
                    resources: Resources::default(),
                    executor: vec![],
                },
                price: PriceRange::default(),
                min: 0,
                target: 1,
                grace: Duration::from_secs(1),
            },
        );
        let parameter_handle = parameter_pool_stream.handle();

        for _ in 0..3 {
            tokio::time::timeout(Duration::from_secs(1), worker_pool_stream.next())
                .await
                .expect("worker pool populate timeout")
                .expect("worker pool ended")
                .expect("worker allocation failed");
            if worker_pool_stream.statistics().len() >= 3 {
                break;
            }
        }

        tokio::time::timeout(Duration::from_secs(1), parameter_pool_stream.next())
            .await
            .expect("parameter pool populate timeout")
            .expect("parameter pool ended")
            .expect("ps allocation failed");

        let (tx, _rx) = tokio::sync::mpsc::channel::<(PeerId, Metrics)>(8);
        let round_state = std::sync::Arc::new(tokio::sync::Mutex::new(RoundState {
            sent_updates: Default::default(),
            first_update_at: None,
            min_quorum: 3,
            grace: Duration::from_millis(0),
            round: 0,
        }));
        let training_state = std::sync::Arc::new(tokio::sync::Mutex::new(TrainingState::new(800)));
        let batch_sizer = std::sync::Arc::new(|resources: &Resources| resources.gpu() as u32);

        // Trace of the intended schedule (counts are remaining samples, times are ms since start):
        // Start: updates [0,0,0], times [0,0,0], cnt=800, t=0
        // W3 Status: updates [0,0,1], times [0,0,500], cnt=750, t=500
        // W2 Status: updates [0,1,1], times [0,800,500], cnt=650, t=800
        // W1 Status: updates [1,1,1], times [950,800,500], cnt=500, t=950 -> schedule W1
        // W3 Status: updates [1,1,2], times [950,800,1000], cnt=450, t=1000
        // W3 Status: updates [1,1,3], times [950,800,1500], cnt=400, t=1500
        // W2 Status: updates [1,2,3], times [950,1600,1500], cnt=300, t=1600 -> schedule W2
        // W1 Status: updates [2,2,3], times [1900,1600,1500], cnt=150, t=1900
        // W3 Status: updates [2,2,4], times [1900,1600,2000], cnt=100, t=2000 -> schedule W3
        // W2 Status: updates [2,3,4], times [1900,2400,2000], cnt=0, t=2400
        // Workers send SentUpdate; PS aggregates, broadcasts; workers resume in next round.

        let start_for = |elapsed_ms: u64| {
            std::time::Instant::now() - std::time::Duration::from_millis(elapsed_ms)
        };

        let steps = vec![
            Step::new(
                w3_id,
                ExecutorStatus::Train(TrainStatus::BatchCompleted { batch_size: 50 }),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                500,
            ),
            Step::new(
                w2_id,
                ExecutorStatus::Train(TrainStatus::BatchCompleted { batch_size: 100 }),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                800,
            ),
            Step::new(
                w1_id,
                ExecutorStatus::Train(TrainStatus::BatchCompleted { batch_size: 150 }),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                950,
            ),
            Step::new(
                w3_id,
                ExecutorStatus::Train(TrainStatus::BatchCompleted { batch_size: 50 }),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                1000,
            ),
            Step::new(
                w3_id,
                ExecutorStatus::Train(TrainStatus::BatchCompleted { batch_size: 50 }),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                1500,
            ),
            Step::new(
                w2_id,
                ExecutorStatus::Train(TrainStatus::BatchCompleted { batch_size: 100 }),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                1600,
            ),
            Step::new(
                w1_id,
                ExecutorStatus::Train(TrainStatus::BatchCompleted { batch_size: 150 }),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                1900,
            ),
            Step::new(
                w3_id,
                ExecutorStatus::Train(TrainStatus::BatchCompleted { batch_size: 50 }),
                hypha_messages::action::ExecutorAction::Train(TrainAction::SendUpdate {
                    target: Reference::Peers {
                        peers: vec![ps_id],
                        strategy: SelectionStrategy::One,
                        resource: None,
                    },
                    timeout: SystemTime::now(),
                }),
                2000,
            ),
            Step::new(
                w2_id,
                ExecutorStatus::Train(TrainStatus::BatchCompleted { batch_size: 100 }),
                hypha_messages::action::ExecutorAction::Train(TrainAction::SendUpdate {
                    target: Reference::Peers {
                        peers: vec![ps_id],
                        strategy: SelectionStrategy::One,
                        resource: None,
                    },
                    timeout: SystemTime::now(),
                }),
                2400,
            ),
            Step::new(
                w1_id,
                ExecutorStatus::Train(TrainStatus::SentUpdate),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ApplyUpdate {
                    source: Reference::Peers {
                        peers: vec![ps_id],
                        strategy: SelectionStrategy::All,
                        resource: None,
                    },
                    timeout: SystemTime::now(),
                }),
                2450,
            ),
            Step::new(
                w2_id,
                ExecutorStatus::Train(TrainStatus::SentUpdate),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ApplyUpdate {
                    source: Reference::Peers {
                        peers: vec![ps_id],
                        strategy: SelectionStrategy::All,
                        resource: None,
                    },
                    timeout: SystemTime::now(),
                }),
                2450,
            ),
            Step::new(
                w3_id,
                ExecutorStatus::Train(TrainStatus::SentUpdate),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ApplyUpdate {
                    source: Reference::Peers {
                        peers: vec![ps_id],
                        strategy: SelectionStrategy::All,
                        resource: None,
                    },
                    timeout: SystemTime::now(),
                }),
                2450,
            ),
            Step::new(
                ps_id,
                ExecutorStatus::Aggregate(AggregateStatus::Idle),
                hypha_messages::action::ExecutorAction::Aggregate(
                    hypha_messages::action::AggregateAction::AggregateUpdates {
                        source: Reference::Peers {
                            peers: vec![w1_id, w2_id, w3_id],
                            strategy: SelectionStrategy::All,
                            resource: None,
                        },
                    },
                ),
                2500,
            ),
            Step::new(
                ps_id,
                ExecutorStatus::Aggregate(AggregateStatus::AggregatedUpdates { metrics: None }),
                hypha_messages::action::ExecutorAction::Aggregate(
                    hypha_messages::action::AggregateAction::BroadcastUpdate {
                        target: Reference::Peers {
                            peers: vec![w1_id, w2_id, w3_id],
                            strategy: SelectionStrategy::All,
                            resource: None,
                        },
                    },
                ),
                2510,
            ),
            Step::new(
                ps_id,
                ExecutorStatus::Aggregate(AggregateStatus::BroadcastedUpdate { metrics: None }),
                hypha_messages::action::ExecutorAction::Aggregate(
                    hypha_messages::action::AggregateAction::Idle {
                        timeout: SystemTime::now(),
                    },
                ),
                2520,
            ),
            Step::new(
                w1_id,
                ExecutorStatus::Train(TrainStatus::AppliedUpdate {
                    round: 0,
                    metrics: HashMap::new(),
                }),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                2530,
            ),
            Step::new(
                w2_id,
                ExecutorStatus::Train(TrainStatus::AppliedUpdate {
                    round: 0,
                    metrics: HashMap::new(),
                }),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                2530,
            ),
            Step::new(
                w3_id,
                ExecutorStatus::Train(TrainStatus::AppliedUpdate {
                    round: 0,
                    metrics: HashMap::new(),
                }),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                2530,
            ),
        ];

        for (idx, step) in steps.iter().enumerate() {
            let resp = schedule::<TestStat, BasicSimulation>(
                tx.clone(),
                worker_handle.clone(),
                parameter_handle.clone(),
                round_state.clone(),
                training_state.clone(),
                batch_sizer.clone(),
                start_for(step.elapsed_ms),
                (
                    step.peer,
                    ActionRequest {
                        job_id: Uuid::new_v4(),
                        status: step.status.clone(),
                    },
                ),
            )
            .await
            .expect("schedule");

            assert_eq!(
                std::mem::discriminant(&resp.next),
                std::mem::discriminant(&step.expect),
                "mismatched action at step {}",
                idx
            );
        }

        {
            let state = training_state.lock().await;
            assert_eq!(
                state.samples_remaining(),
                800,
                "counter reset for new round"
            );
        }
        {
            let state = round_state.lock().await;
            assert_eq!(state.round, 1, "round advanced after broadcast");
            assert!(
                state.sent_updates.is_empty(),
                "sent updates cleared after broadcast"
            );
        }

        // Workers acknowledge the broadcast with AppliedUpdate and should resume executing batches.
        for peer in [w1_id, w2_id, w3_id] {
            let resp = schedule::<TestStat, BasicSimulation>(
                tx.clone(),
                worker_handle.clone(),
                parameter_handle.clone(),
                round_state.clone(),
                training_state.clone(),
                batch_sizer.clone(),
                start_for(2600),
                (
                    peer,
                    ActionRequest {
                        job_id: Uuid::new_v4(),
                        status: ExecutorStatus::Train(TrainStatus::AppliedUpdate {
                            round: 0,
                            metrics: HashMap::new(),
                        }),
                    },
                ),
            )
            .await
            .expect("applied update");
            match resp.next {
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch) => {}
                other => panic!("Expected ExecuteBatch, got {:?}", other),
            }
        }
    }
}