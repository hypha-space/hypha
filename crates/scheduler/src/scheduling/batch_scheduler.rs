use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use hypha_messages::{
    Reference, SelectionStrategy,
    action::{
        self, AggregateAction, AggregateError, AggregateStatus, ExecutorAction, ExecutorStatus,
        TrainAction, TrainError, TrainStatus,
    },
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
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    metrics_bridge::Metrics,
    network::Network,
    pool::{PoolHandle, PoolStatisticsHandle},
    scheduler_config::ModelDestiantion,
    simulation::Simulation,
    statistics::RuntimeStatistic,
};

// NOTE: Tracks per-round update signals from workers so the scheduler can
// decide when to instruct the parameter server to aggregate.
struct RoundState {
    aggregated_updates: bool,
    sent_updates: HashSet<PeerId>,
    first_update_at: Option<Instant>,
    round_started_at: Instant,
    aggregate_started_at: Option<Instant>,
    min_quorum: usize,
    grace: Duration,
    round: u32,
    update_rounds: u32,
    push_assigned: Option<PeerId>,
    training_complete: bool,
    applied_final_update: HashSet<PeerId>,
    push_done: bool,
    // NOTE: Tracks workers that have applied the update for the current round.
    applied_updates: HashSet<PeerId>,
}

impl Default for RoundState {
    fn default() -> Self {
        Self {
            aggregated_updates: false,
            sent_updates: HashSet::new(),
            first_update_at: None,
            round_started_at: Instant::now(),
            aggregate_started_at: None,
            min_quorum: 0,
            grace: Duration::from_millis(0),
            round: 0,
            update_rounds: 0,
            push_assigned: None,
            training_complete: false,
            applied_final_update: HashSet::new(),
            push_done: false,
            applied_updates: HashSet::new(),
        }
    }
}

#[derive(Default)]
struct TrainingState {
    update_target: u32,
    counter: u32,
    peer_updates: HashMap<PeerId, u32>,
    worker_without_model: Vec<PeerId>,
    receive_from: HashMap<PeerId, PeerId>,
}

impl TrainingState {
    fn new(update_target: u32) -> Self {
        Self {
            update_target,
            counter: 0,
            peer_updates: HashMap::new(),
            worker_without_model: vec![],
            receive_from: HashMap::new(),
        }
    }

    fn record_batch(&mut self, batch_size: u32, peer_id: PeerId) {
        self.counter = self.counter.saturating_add(batch_size);
        match self.peer_updates.entry(peer_id) {
            Entry::Occupied(mut entry) => {
                let processed = entry.get();
                entry.insert(processed.saturating_add(batch_size));
            }
            Entry::Vacant(entry) => {
                entry.insert(batch_size);
            }
        }
    }

    fn get_count(&self) -> u32 {
        self.counter
    }

    fn get_update_target(&self) -> u32 {
        self.update_target
    }

    fn reset_round(&mut self) {
        self.counter = 0;
        self.peer_updates = HashMap::new();
    }

    fn get_peer_updates(&self, peer_id: &PeerId) -> u32 {
        *self.peer_updates.get(peer_id).unwrap_or(&0u32)
    }

    fn pop_worker_without_model(&mut self) -> Option<PeerId> {
        self.worker_without_model.pop()
    }

    fn push_worker_without_model(&mut self, peer_id: PeerId) {
        self.worker_without_model.push(peer_id);
    }

    fn remove_receive_from(&mut self, peer_id: &PeerId) -> Option<PeerId> {
        self.receive_from.remove(peer_id)
    }

    fn insert_receive_from(&mut self, source: PeerId, destination: PeerId) {
        self.receive_from.insert(destination, source);
    }

    fn get_waiting_workers(&self) -> &[PeerId] {
        &self.worker_without_model[..]
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
#[allow(clippy::too_many_arguments)]
async fn schedule<T, S>(
    tx: Sender<(PeerId, Metrics)>,
    worker_pool: PoolStatisticsHandle<T>,
    parameter_pool: PoolHandle,
    round_state: Arc<Mutex<RoundState>>,
    training_state: Arc<Mutex<TrainingState>>,
    batch_sizer: BatchSizer,
    push_destination: Arc<Option<ModelDestiantion>>,
    start: std::time::Instant,
    request: (PeerId, action::ActionRequest),
    cancel: CancellationToken,
) -> Result<action::ActionResponse, BatchSchedulerError>
where
    T: RuntimeStatistic + 'static,
    S: Simulation + Send + Sync + 'static,
{
    let _ = std::marker::PhantomData::<S>;
    let (peer_id, action::ActionRequest { job_id, status }) = request;
    tracing::debug!(%peer_id, ?status, %job_id, "Received action request");

    let now = SystemTime::now();
    // Rimeouts sized for ~100ms RTT with generous margins.
    let short_idle = now + Duration::from_millis(500);
    let wait_model = now + Duration::from_secs(1);
    let long_io = now + Duration::from_secs(60);
    let ps_broadcast_idle = now + Duration::from_secs(5);

    let since_start = start.elapsed().as_millis() as u64;
    // NOTE: We rely on Pool::members() being oldest-first ordered by join time.
    let parameter_servers: Vec<PeerId> = parameter_pool.iter().map(|w| w.peer_id).collect();
    let primary_ps = parameter_servers.first().copied();

    let next_action = match status {
        ExecutorStatus::Train(train) => match train {
            TrainStatus::Joined => {
                let state = round_state.lock().await;
                if state.round == 0 {
                    ExecutorAction::Train(TrainAction::Idle {
                        timeout: short_idle,
                    })
                } else {
                    training_state
                        .lock()
                        .await
                        .push_worker_without_model(peer_id);
                    ExecutorAction::Train(TrainAction::WaitForModel {
                        timeout: wait_model,
                    })
                }
            }
            TrainStatus::WaitedForModel => {
                if let Some(sending_peer) =
                    training_state.lock().await.remove_receive_from(&peer_id)
                {
                    ExecutorAction::Train(TrainAction::ReceiveModel {
                        source: Reference::Peers {
                            peers: vec![sending_peer],
                            strategy: SelectionStrategy::All,
                            resource: None,
                        },
                        timeout: long_io,
                    })
                } else {
                    ExecutorAction::Train(TrainAction::WaitForModel {
                        timeout: wait_model,
                    })
                }
            }
            TrainStatus::ReceivedModel => {
                // Lazy transition to other state
                ExecutorAction::Train(TrainAction::Idle { timeout: now })
            }
            TrainStatus::SentModel => {
                // Lazy transition to other state
                ExecutorAction::Train(TrainAction::Idle { timeout: now })
            }
            TrainStatus::Idle => {
                let mut state = round_state.lock().await;
                if !state.training_complete {
                    let snapshot = worker_pool.statistics();
                    let peer_position = snapshot
                        .iter()
                        .position(|w| w.peer_id == peer_id)
                        .unwrap_or(0);

                    let (count, update_target, peer_contribution) = {
                        let training = training_state.lock().await;
                        (
                            training.get_count(),
                            training.get_update_target(),
                            training.get_peer_updates(&peer_id),
                        )
                    };

                    let stats: Vec<u64> =
                        snapshot.iter().map(|w| w.statistic.unwrap_or(0)).collect();
                    let progress: Vec<u64> = snapshot
                        .iter()
                        .map(|w| w.last_updated.unwrap_or(0))
                        .collect();
                    let batch_sizes: Vec<u32> = snapshot
                        .iter()
                        .map(|w| (batch_sizer)(&w.resources))
                        .collect();

                    let (should_update, projected_target) = if update_target <= count {
                        (true, count)
                    } else if !snapshot.is_empty()
                        && batch_sizes.iter().all(|&b| b > 0)
                        && stats.iter().all(|&s| s > 0 && s < u64::MAX)
                    {
                        let (time, cnt, projection, capped) = S::project(
                            &progress,
                            &batch_sizes,
                            stats,
                            update_target.saturating_sub(count),
                            SIM_TIME_CAP_MS,
                            SIM_UPDATE_CAP,
                        );

                        tracing::debug!(
                            time = %time,
                            count = %cnt,
                            peer = %peer_id,
                            "Simulation with projection {:?} and {:?}",
                            projection,
                            update_target.saturating_sub(count)
                        );
                        (
                            cnt <= 0
                                && !capped
                                && peer_position < projection.len()
                                && projection[peer_position] == 0,
                            count.saturating_add(cnt.unsigned_abs()),
                        )
                    } else {
                        (false, count)
                    };

                    if state.aggregated_updates && !state.applied_updates.contains(&peer_id) {
                        ExecutorAction::Train(TrainAction::ApplyUpdate {
                            source: Reference::Peers {
                                peers: parameter_servers,
                                strategy: SelectionStrategy::All,
                                resource: None,
                            },
                            timeout: now + Duration::from_secs(10),
                        })
                    } else if state.sent_updates.contains(&peer_id) {
                        ExecutorAction::Train(TrainAction::Idle {
                            timeout: short_idle,
                        })
                    } else if !should_update {
                        ExecutorAction::Train(TrainAction::ExecuteBatch)
                    } else if parameter_servers.is_empty() {
                        // NOTE: If we need to send an update but there are no parameter servers,
                        // we must wait (idle) until one becomes available.
                        ExecutorAction::Train(TrainAction::Idle {
                            timeout: short_idle,
                        })
                    } else {
                        ExecutorAction::Train(TrainAction::SendUpdate {
                            target: Reference::Peers {
                                // Selecting a single PS to avoid that workers send updates to multiple PS
                                peers: vec![parameter_servers[0]],
                                strategy: SelectionStrategy::One,
                                resource: None,
                            },
                            weight: peer_contribution as f32 / projected_target as f32,
                        })
                    }
                } else if state.push_done {
                    cancel.cancel();
                    ExecutorAction::Train(TrainAction::Terminate)
                } else if state.push_assigned.is_none()
                    && state.applied_final_update.contains(&peer_id)
                {
                    state.push_assigned = Some(peer_id);
                    if let Some(destination) = push_destination.as_ref().as_ref() {
                        ExecutorAction::Train(TrainAction::PushToHub {
                            repository: destination.repository.clone(),
                            token: destination.token.clone(),
                        })
                    } else {
                        // Should not occur due to guard above.
                        state.push_done = true;
                        ExecutorAction::Train(TrainAction::Terminate)
                    }
                } else {
                    ExecutorAction::Train(TrainAction::Idle {
                        timeout: short_idle,
                    })
                }
            }
            TrainStatus::BatchCompleted { batch_size } => {
                worker_pool.update(&peer_id, since_start);

                let snapshot = worker_pool.statistics();
                let peer_position = snapshot
                    .iter()
                    .position(|w| w.peer_id == peer_id)
                    .unwrap_or(0);

                let (count, update_target, peer_contribution) = {
                    let mut training = training_state.lock().await;
                    training.record_batch(batch_size, peer_id);
                    (
                        training.get_count(),
                        training.get_update_target(),
                        training.get_peer_updates(&peer_id),
                    )
                };

                let (training_complete, sent_update) = {
                    let state = round_state.lock().await;
                    (
                        state.training_complete,
                        state.sent_updates.contains(&peer_id),
                    )
                };

                if training_complete || sent_update {
                    ExecutorAction::Train(TrainAction::Idle {
                        timeout: short_idle,
                    })
                } else {
                    let stats: Vec<u64> =
                        snapshot.iter().map(|w| w.statistic.unwrap_or(0)).collect();
                    let progress: Vec<u64> = snapshot
                        .iter()
                        .map(|w| w.last_updated.unwrap_or(0))
                        .collect();
                    let batch_sizes: Vec<u32> = snapshot
                        .iter()
                        .map(|w| (batch_sizer)(&w.resources))
                        .collect();

                    let (should_update, projected_target) = if update_target <= count {
                        (true, count)
                    } else if !snapshot.is_empty()
                        && batch_sizes.iter().all(|&b| b > 0)
                        && stats.iter().all(|&s| s > 0 && s < u64::MAX)
                    {
                        let (time, cnt, projection, capped) = S::project(
                            &progress,
                            &batch_sizes,
                            stats,
                            update_target.saturating_sub(count),
                            SIM_TIME_CAP_MS,
                            SIM_UPDATE_CAP,
                        );

                        tracing::debug!(
                            time = %time,
                            count = %cnt,
                            peer = %peer_id,
                            "Simulation with projection {:?} and {:?}",
                            projection,
                            update_target.saturating_sub(count)
                        );
                        (
                            cnt <= 0
                                && !capped
                                && peer_position < projection.len()
                                && projection[peer_position] == 0,
                            count.saturating_add(cnt.unsigned_abs()),
                        )
                    } else {
                        (false, count)
                    };

                    if !should_update {
                        ExecutorAction::Train(TrainAction::ExecuteBatch)
                    } else if parameter_servers.is_empty() {
                        // NOTE: If we need to send an update but there are no parameter servers,
                        // we must wait (idle) until one becomes available.
                        ExecutorAction::Train(TrainAction::Idle {
                            timeout: short_idle,
                        })
                    } else {
                        ExecutorAction::Train(TrainAction::SendUpdate {
                            target: Reference::Peers {
                                // Selecting a single PS to avoid that workers send updates to multiple PS
                                peers: vec![parameter_servers[0]],
                                strategy: SelectionStrategy::One,
                                resource: None,
                            },
                            weight: peer_contribution as f32 / projected_target as f32,
                        })
                    }
                }
            }
            TrainStatus::SentUpdate { mut metrics, .. } => {
                let worker_samples = {
                    let training = training_state.lock().await;
                    training.get_peer_updates(&peer_id) as f32
                };

                let (round, round_started_at) = {
                    let state = round_state.lock().await;
                    (state.round, state.round_started_at)
                };

                let elapsed_secs = round_started_at.elapsed().as_secs_f32();
                let worker_steps_per_sec = if elapsed_secs > 0.0 {
                    worker_samples / elapsed_secs
                } else {
                    0.0
                };

                metrics.insert("round".to_string(), round as f32);
                metrics.insert("data_points".to_string(), worker_samples as f32);
                metrics.insert("steps".to_string(), worker_steps_per_sec as f32);
                metrics.insert("duration".to_string(), elapsed.as_secs_f32());

                tx.send((peer_id, Metrics { round, metrics }))
                    .await
                    .map_err(BatchSchedulerError::from)?;
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

                ExecutorAction::Train(TrainAction::Idle {
                    timeout: short_idle,
                })
            }
            TrainStatus::AppliedUpdate => {
                let training_complete = {
                    let mut state = round_state.lock().await;
                    state.applied_updates.insert(peer_id);

                    if state.training_complete {
                        state.applied_final_update.insert(peer_id);
                        true
                    } else {
                        false
                    }
                };

                if training_complete {
                    ExecutorAction::Train(TrainAction::Idle {
                        timeout: now + Duration::from_millis(500),
                    })
                } else {
                    let mut training = training_state.lock().await;
                    if let Some(update_worker) = training.pop_worker_without_model() {
                        training.insert_receive_from(peer_id, update_worker);
                        ExecutorAction::Train(TrainAction::SendModel {
                            target: Reference::Peers {
                                peers: vec![update_worker],
                                strategy: SelectionStrategy::One,
                                resource: None,
                            },
                        })
                    } else {
                        ExecutorAction::Train(TrainAction::ExecuteBatch)
                    }
                }
            }
            TrainStatus::PushedToHub => {
                let mut state = round_state.lock().await;
                if state.push_assigned == Some(peer_id) {
                    state.push_done = true;
                }

                ExecutorAction::Train(TrainAction::Terminate)
            }
            TrainStatus::Error(TrainError::Connection { message }) => {
                tracing::warn!(%peer_id, message = %message, "Worker reported connection error");
                {
                    let mut state = round_state.lock().await;
                    if state.push_assigned == Some(peer_id) {
                        state.push_assigned = None;
                    }
                }
                ExecutorAction::Train(TrainAction::Idle {
                    timeout: short_idle,
                })
            }
            TrainStatus::Error(TrainError::Other { message }) => {
                tracing::warn!(%peer_id, message = %message, "Worker reported error");
                {
                    let mut state = round_state.lock().await;
                    if state.push_assigned == Some(peer_id) {
                        state.push_assigned = None;
                    }
                }
                ExecutorAction::Train(TrainAction::Terminate)
            }
            TrainStatus::Terminated => ExecutorAction::Train(TrainAction::Terminate),
        },
        ExecutorStatus::Aggregate(state) => match state {
            AggregateStatus::Idle => {
                let training_complete = { round_state.lock().await.training_complete };
                if training_complete {
                    ExecutorAction::Aggregate(AggregateAction::Terminate)
                } else if Some(peer_id) != primary_ps {
                    if let Some(primary) = primary_ps {
                        tracing::debug!(
                            %peer_id,
                            primary_ps = %primary,
                            "Non-primary PS polling; returning Idle"
                        );
                    }
                    ExecutorAction::Aggregate(AggregateAction::Idle {
                        timeout: short_idle,
                    })
                } else {
                    let workers: Vec<_> = {
                        let training = training_state.lock().await;
                        let non_participating_worker = training.get_waiting_workers();
                        worker_pool
                            .statistics()
                            .into_iter()
                            .map(|w| w.peer_id)
                            .filter(|w| !non_participating_worker.contains(w))
                            .collect()
                    };

                    if workers.is_empty() {
                        ExecutorAction::Aggregate(AggregateAction::Idle {
                            timeout: short_idle,
                        })
                    } else {
                        // Start aggregation when either all workers have sent updates,
                        // or when a quorum (min workers) have sent updates and the
                        // grace period has elapsed since the first update in this round.
                        let mut state = round_state.lock().await;
                        let all_sent = workers.iter().all(|w| state.sent_updates.contains(w));
                        let effective_quorum = state.min_quorum.min(workers.len());
                        let quorum_met = state.sent_updates.len() >= effective_quorum;
                        let timebox_elapsed = state
                            .first_update_at
                            .map(|t| t.elapsed() >= state.grace)
                            .unwrap_or(false);
                        let ready = all_sent || (quorum_met && timebox_elapsed);

                        if ready {
                            if state.aggregate_started_at.is_none() {
                                state.aggregate_started_at = Some(Instant::now());
                            }
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
                                timeout: short_idle,
                            })
                        }
                    }
                }
            }
            AggregateStatus::AggregatedUpdates { .. } => {
                // Only allow the primary PS to proceed to broadcast.
                if Some(peer_id) != primary_ps {
                    ExecutorAction::Aggregate(AggregateAction::Idle {
                        timeout: ps_broadcast_idle,
                    })
                } else {
                    let workers: Vec<_> = worker_pool
                        .statistics()
                        .into_iter()
                        .map(|w| w.peer_id)
                        .collect();

                    if workers.is_empty() {
                        ExecutorAction::Aggregate(AggregateAction::Idle {
                            timeout: short_idle,
                        })
                    } else {
                        // Log that we are moving to broadcast for this round.
                        let round = {
                            let mut state = round_state.lock().await;
                            state.aggregated_updates = true;
                            state.applied_updates.clear();
                            state.round
                        };
                        tracing::info!(round = %round, "Trigger BroadcastUpdate");

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
                let total_samples = {
                    let training = training_state.lock().await;
                    training.get_count() as f32
                };

                let (round_number, round_started_at) = {
                    let state = round_state.lock().await;
                    (state.round, state.round_started_at)
                };

                let mut metrics = metrics.unwrap_or_default();
                let round_duration_secs = round_started_at.elapsed().as_secs_f32();
                let steps_per_sec = if round_duration_secs > 0.0 {
                    total_samples / round_duration_secs
                } else {
                    0.0
                };

                metrics.insert("round".to_string(), round_number as f32);
                metrics.insert("data_points".to_string(), total_samples);
                metrics.insert("steps".to_string(), steps_per_sec);
                metrics.insert("duration".to_string(), round_duration_secs);

                tx.send((
                    peer_id,
                    Metrics {
                        round: round_number,
                        metrics,
                    },
                ))
                .await
                .map_err(BatchSchedulerError::from)?;

                let next_round = {
                    let mut state = round_state.lock().await;

                    tracing::info!(round = state.round, "Broadcast completed; advancing round");

                    state.sent_updates.clear();
                    state.first_update_at = None;
                    state.aggregate_started_at = None;
                    state.round_started_at = Instant::now();
                    state.round = state.round.saturating_add(1);

                    if state.round >= state.update_rounds {
                        state.training_complete = true;
                        tracing::info!(
                            round = state.round,
                            target = state.update_rounds,
                            "Target update rounds reached; entering completion phase"
                        );
                    }

                    state.round
                };

                let mut training = training_state.lock().await;
                training.reset_round();
                tracing::info!(
                    round = next_round,
                    "Next round started; training state reset"
                );

                let training_complete = {
                    let mut state = round_state.lock().await;
                    state.aggregated_updates = false;
                    state.training_complete
                };
                if training_complete {
                    ExecutorAction::Aggregate(AggregateAction::Terminate)
                } else {
                    ExecutorAction::Aggregate(AggregateAction::Idle {
                        timeout: short_idle,
                    })
                }
            }
            AggregateStatus::Error(AggregateError::Connection { message }) => {
                tracing::warn!(%peer_id, message = %message, "Aggregator reported connection error");
                {
                    let mut state = round_state.lock().await;
                    state.aggregated_updates = false;
                }
                ExecutorAction::Aggregate(AggregateAction::Idle {
                    timeout: short_idle,
                })
            }
            AggregateStatus::Error(AggregateError::Other { message }) => {
                tracing::warn!(%peer_id, message = %message, "Aggregator reported error");
                {
                    let mut state = round_state.lock().await;
                    state.aggregated_updates = false;
                }
                ExecutorAction::Aggregate(AggregateAction::Terminate)
            }

            AggregateStatus::Terminated => ExecutorAction::Aggregate(AggregateAction::Terminate),
        },
    };

    tracing::debug!(%peer_id, %job_id, response = ?next_action, "Sending action response");

    Ok(action::ActionResponse {
        job_id,
        next: next_action,
    })
}

pub struct BatchScheduler {}

impl BatchScheduler {
    // TODO: Consider deriving `min_quorum` and `grace` from pool values as we should be ok to use the pool min size and pool grace period here as well.
    #[allow(clippy::too_many_arguments)]
    pub async fn run<T, S>(
        network: Network,
        worker_pool: PoolStatisticsHandle<T>,
        parameter_pool: PoolHandle,
        id: Uuid,
        min_quorum: usize,
        grace: Duration,
        samples_between_updates: u32,
        update_rounds: u32,
        push_destination: Option<ModelDestiantion>,
        batch_sizer: BatchSizer,
        cancel: CancellationToken,
    ) -> Result<(mpsc::Receiver<(PeerId, Metrics)>, JoinHandle<()>), BatchSchedulerError>
    where
        T: RuntimeStatistic + 'static,
        S: Simulation + Send + Sync + 'static,
    {
        let _ = std::marker::PhantomData::<S>;
        let (tx, rx) = mpsc::channel(100);
        let start = std::time::Instant::now();
        let push_destination = Arc::new(push_destination);
        let stream_handle = tokio::spawn({
            let worker_pool = worker_pool.clone();
            let parameter_pool = parameter_pool.clone();
            let batch_sizer = batch_sizer.clone();
            let push_destination = push_destination.clone();
            // NOTE: Track per-round SentUpdate signals to decide when to trigger aggregation.
            let round_state = Arc::new(Mutex::new(RoundState {
                sent_updates: HashSet::new(),
                first_update_at: None,
                round_started_at: start,
                aggregate_started_at: None,
                min_quorum,
                grace,
                round: 0,
                update_rounds,
                push_assigned: None,
                training_complete: false,
                applied_final_update: HashSet::new(),
                push_done: false,
                aggregated_updates: false,
                applied_updates: HashSet::new(),
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
                    let push_destination = push_destination.clone();
                    let cancel = cancel.clone();
                    async move {
                        match schedule::<T, S>(
                            tx,
                            worker_pool,
                            parameter_pool,
                            round_state,
                            training_state,
                            batch_sizer,
                            push_destination,
                            start,
                            request,
                            cancel,
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
    use std::{
        collections::{HashMap, HashSet},
        time::{Instant, SystemTime},
    };

    use futures_util::StreamExt;
    use hypha_messages::{
        Reference, SelectionStrategy,
        action::{
            ActionRequest, AggregateStatus, ExecutorAction, ExecutorStatus, TrainAction,
            TrainStatus,
        },
    };
    use hypha_resources::Resources;
    use libp2p::PeerId;
    use tokio::time::Duration;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    use super::{RoundState, TrainingState, schedule};
    use crate::{
        allocator::{Allocator, AllocatorError},
        metrics_bridge::Metrics,
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
        let push_destination = std::sync::Arc::new(None);
        let token = CancellationToken::new();
        let resp = schedule::<RunningMean, BasicSimulation>(
            tx,
            worker_handle,
            parameter_pool,
            round,
            training_state,
            batch_sizer,
            push_destination,
            std::time::Instant::now(),
            (
                PeerId::random(),
                ActionRequest {
                    job_id: Uuid::new_v4(),
                    status: ExecutorStatus::Train(TrainStatus::Idle),
                },
            ),
            token.clone(),
        )
        .await
        .unwrap();

        match resp.next {
            hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch) => {}
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[tokio::test]
    async fn train_continues_batching_without_parameter_server() {
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
        let training_state = std::sync::Arc::new(tokio::sync::Mutex::new(TrainingState::new(100)));
        let batch_sizer = std::sync::Arc::new(|_: &Resources| 1u32);
        let push_destination = std::sync::Arc::new(None);
        let token = CancellationToken::new();
        let resp = schedule::<RunningMean, BasicSimulation>(
            tx,
            worker_handle,
            parameter_pool,
            round,
            training_state,
            batch_sizer,
            push_destination,
            std::time::Instant::now(),
            (
                PeerId::random(),
                ActionRequest {
                    job_id: Uuid::new_v4(),
                    status: ExecutorStatus::Train(TrainStatus::BatchCompleted { batch_size: 4 }),
                },
            ),
            token.clone(),
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
        let push_destination = std::sync::Arc::new(None);
        let token = CancellationToken::new();
        let resp = schedule::<RunningMean, BasicSimulation>(
            tx,
            worker_handle,
            parameter_pool,
            round,
            training_state,
            batch_sizer,
            push_destination,
            std::time::Instant::now(),
            (
                PeerId::random(),
                ActionRequest {
                    job_id: Uuid::new_v4(),
                    status: ExecutorStatus::Train(TrainStatus::BatchCompleted { batch_size: 10 }),
                },
            ),
            token.clone(),
        )
        .await
        .unwrap();

        match resp.next {
            hypha_messages::action::ExecutorAction::Train(TrainAction::Idle { .. }) => {}
            other => panic!("Unexpected response: {:?}", other),
        }
    }

    #[tokio::test]
    async fn train_idle_respects_samples_remaining() {
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
        // Initialize with 0 samples remaining to simulate end of round
        let training_state = std::sync::Arc::new(tokio::sync::Mutex::new(TrainingState::new(0)));
        let batch_sizer = std::sync::Arc::new(|_: &Resources| 1u32);
        let push_destination = std::sync::Arc::new(None);
        let token = CancellationToken::new();
        let resp = schedule::<RunningMean, BasicSimulation>(
            tx,
            worker_handle,
            parameter_pool,
            round,
            training_state,
            batch_sizer,
            push_destination,
            std::time::Instant::now(),
            (
                PeerId::random(),
                ActionRequest {
                    job_id: Uuid::new_v4(),
                    status: ExecutorStatus::Train(TrainStatus::Idle),
                },
            ),
            token.clone(),
        )
        .await
        .unwrap();

        match resp.next {
            // Should be Idle because samples == 0 and no PS, not ExecuteBatch
            hypha_messages::action::ExecutorAction::Train(TrainAction::Idle { .. }) => {}
            other => panic!("Expected Idle when samples=0 and no PS, got: {:?}", other),
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
            round_started_at: Instant::now(),
            aggregate_started_at: None,
            min_quorum: 3,
            grace: Duration::from_millis(0),
            round: 0,
            update_rounds: 10,
            push_assigned: None,
            training_complete: false,
            applied_final_update: Default::default(),
            push_done: false,
            aggregated_updates: false,
            applied_updates: HashSet::default(),
        }));
        let training_state = std::sync::Arc::new(tokio::sync::Mutex::new(TrainingState::new(800)));
        let batch_sizer = std::sync::Arc::new(|resources: &Resources| resources.gpu() as u32);
        let push_destination = std::sync::Arc::new(None);

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
                    weight: 0.3,
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
                    weight: 0.3,
                }),
                2400,
            ),
            Step::new(
                w1_id,
                ExecutorStatus::Train(TrainStatus::SentUpdate {
                    round: 0,
                    metrics: HashMap::new(),
                }),
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
                ExecutorStatus::Train(TrainStatus::SentUpdate {
                    round: 0,
                    metrics: HashMap::new(),
                }),
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
                ExecutorStatus::Train(TrainStatus::SentUpdate {
                    round: 0,
                    metrics: HashMap::new(),
                }),
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
                ExecutorStatus::Train(TrainStatus::AppliedUpdate),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                2530,
            ),
            Step::new(
                w2_id,
                ExecutorStatus::Train(TrainStatus::AppliedUpdate),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                2530,
            ),
            Step::new(
                w3_id,
                ExecutorStatus::Train(TrainStatus::AppliedUpdate),
                hypha_messages::action::ExecutorAction::Train(TrainAction::ExecuteBatch),
                2530,
            ),
        ];

        for (idx, step) in steps.iter().enumerate() {
            let token = CancellationToken::new();
            let resp = schedule::<TestStat, BasicSimulation>(
                tx.clone(),
                worker_handle.clone(),
                parameter_handle.clone(),
                round_state.clone(),
                training_state.clone(),
                batch_sizer.clone(),
                push_destination.clone(),
                start_for(step.elapsed_ms),
                (
                    step.peer,
                    ActionRequest {
                        job_id: Uuid::new_v4(),
                        status: step.status.clone(),
                    },
                ),
                token.clone(),
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
                state.get_update_target().saturating_sub(state.get_count()),
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
            let token = CancellationToken::new();
            let resp = schedule::<TestStat, BasicSimulation>(
                tx.clone(),
                worker_handle.clone(),
                parameter_handle.clone(),
                round_state.clone(),
                training_state.clone(),
                batch_sizer.clone(),
                push_destination.clone(),
                start_for(2600),
                (
                    peer,
                    ActionRequest {
                        job_id: Uuid::new_v4(),
                        status: ExecutorStatus::Train(TrainStatus::AppliedUpdate),
                    },
                ),
                token.clone(),
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
