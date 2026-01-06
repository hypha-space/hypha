use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use hypha_messages::{
    Reference, SelectionStrategy,
    action::{
        self, AggregateAction, AggregateError, AggregateStatus, ExecutorAction, ExecutorStatus,
        GymnasiumAction, GymnasiumError, GymnasiumStatus, RlTrainAction, RlTrainStatus,
        TrainAction, TrainError,
    },
};
use hypha_network::request_response::{RequestResponseError, RequestResponseInterfaceExt};
use hypha_resources::Resources;
use libp2p::PeerId;
use thiserror::Error;
use tokio::{
    sync::{Mutex, mpsc},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    metrics_bridge::Metrics,
    network::Network,
    pool::{PoolWithAggregateInfoHandle, PoolWithTrainInfoHandle},
    scheduler_config::ModelDestination,
    simulation::Simulation,
    statistics::RuntimeStatistic,
};

// NOTE: Tracks per-round update signals from workers so the scheduler can
// decide when to instruct the parameter server to aggregate.
struct RoundState {
    aggregated_updates: bool,
    first_update_at: Option<Instant>,
    round_started_at: Instant,
    aggregate_started_at: Option<Instant>,
    min_quorum: usize,
    grace: Duration,
    round: u32,
    update_rounds: u32,
    training_complete: bool,
    push_done: bool,
}

impl Default for RoundState {
    fn default() -> Self {
        Self {
            aggregated_updates: false,
            first_update_at: None,
            round_started_at: Instant::now(),
            aggregate_started_at: None,
            min_quorum: 0,
            grace: Duration::from_millis(0),
            round: 0,
            update_rounds: 0,
            training_complete: false,
            push_done: false,
        }
    }
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
            counter: 0,
        }
    }

    fn record_batch(&mut self, batch_size: u32) {
        self.counter = self.counter.saturating_add(batch_size);
    }

    fn get_count(&self) -> u32 {
        self.counter
    }

    fn get_update_target(&self) -> u32 {
        self.update_target
    }

    fn reset_round(&mut self) {
        self.counter = 0;
    }
}

type BatchSizer = Arc<dyn Fn(&Resources) -> u32 + Send + Sync>;

#[derive(Debug, Error)]
pub enum RLSchedulerError {
    #[error("Disconnected")]
    Disconnected,
    #[error("Network error: {0}")]
    NetworkError(#[from] RequestResponseError),
    #[error("Send Metrics Error: {0}")]
    SendMetricsError(#[from] mpsc::error::SendError<(PeerId, Metrics)>),
}

/// Handle action protocol requests and respond with next steps.
#[allow(clippy::too_many_arguments)]
async fn schedule<T, S>(
    tx: mpsc::Sender<(PeerId, Metrics)>,
    gymnasium_pool: PoolWithTrainInfoHandle<T>,
    trainer_pool: PoolWithTrainInfoHandle<T>,
    parameter_pool: PoolWithAggregateInfoHandle,
    round_state: Arc<Mutex<RoundState>>,
    training_state: Arc<Mutex<TrainingState>>,
    batch_sizer: BatchSizer,
    multi_batch_size: u32,
    push_destination: Arc<Option<ModelDestination>>,
    start: std::time::Instant,
    request: (PeerId, action::ActionRequest),
    cancel: CancellationToken,
) -> Result<action::ActionResponse, RLSchedulerError>
where
    T: RuntimeStatistic + 'static,
    S: Simulation + Send + Sync + 'static,
{
    let (peer_id, action::ActionRequest { job_id, status }) = request;
    tracing::debug!(%peer_id, ?status, %job_id, "Received action request");

    let now = SystemTime::now();
    // Rimeouts sized for ~100ms RTT with generous margins.
    let short_idle = now + Duration::from_millis(500);
    let wait_model = now + Duration::from_secs(1);
    let long_io = now + Duration::from_secs(60);
    let ps_broadcast_idle = now + Duration::from_secs(5);

    let since_start = start.elapsed().as_millis() as u64;
    // NOTE: Keep aggregate info aligned with active parameter servers.
    parameter_pool.info();
    // NOTE: We rely on Pool::members() being oldest-first ordered by join time.
    let parameter_servers: Vec<_> = parameter_pool.info().iter().map(|w| w.peer_id).collect();
    let primary_ps = parameter_servers.first().copied();

    let trainer_servers: Vec<_> = trainer_pool.info().iter().map(|w| w.peer_id).collect();

    let next_action = match status {
        ExecutorStatus::Train(_) => {
            return Err(RLSchedulerError::NetworkError(RequestResponseError::Other(
                "Train status is not supported".to_string(),
            )));
        }
        ExecutorStatus::Gymnasium(gymnasium_status) => match gymnasium_status {
            GymnasiumStatus::Joined => {
                let state = round_state.lock().await;
                if state.round == 0 {
                    ExecutorAction::Gymnasium(GymnasiumAction::Idle {
                        timeout: short_idle,
                    })
                } else {
                    gymnasium_pool.update_state(&peer_id, |s| s.waiting_for_model = true);
                    ExecutorAction::Gymnasium(GymnasiumAction::Generate {})
                }
            }
            GymnasiumStatus::Idle => {
                let state = round_state.lock().await;
                if !state.training_complete {
                    ExecutorAction::Gymnasium(GymnasiumAction::Generate {})
                } else {
                    ExecutorAction::Gymnasium(GymnasiumAction::Terminate)
                }
            }
            GymnasiumStatus::GeneratedData => ExecutorAction::Gymnasium(GymnasiumAction::Send {
                // TODO: only sent once trainers are ready to receive
                target: Reference::Peers {
                    peers: trainer_servers,
                    strategy: SelectionStrategy::All,
                    resource: None,
                },
            }),
            GymnasiumStatus::SentData => ExecutorAction::Gymnasium(GymnasiumAction::Idle {
                timeout: short_idle,
            }),
            GymnasiumStatus::ReceivedAgentState => {
                ExecutorAction::Gymnasium(GymnasiumAction::Idle {
                    timeout: short_idle,
                })
            }
            GymnasiumStatus::Error(GymnasiumError::Connection { message }) => {
                tracing::warn!(%peer_id, message = %message, "Gymnasium reported connection error");
                {
                    let mut state = round_state.lock().await;
                    state.aggregated_updates = false;
                }
                ExecutorAction::Gymnasium(GymnasiumAction::Idle {
                    timeout: short_idle,
                })
            }
            GymnasiumStatus::Error(GymnasiumError::Other { message }) => {
                tracing::warn!(%peer_id, message = %message, "Gymnasium reported error");
                {
                    let mut state = round_state.lock().await;
                    state.aggregated_updates = false;
                }
                ExecutorAction::Gymnasium(GymnasiumAction::Terminate)
            }
        },
        ExecutorStatus::RlTrain(train) => match train {
            RlTrainStatus::Joined => {
                let state = round_state.lock().await;
                if state.round == 0 {
                    ExecutorAction::Train(TrainAction::Idle {
                        timeout: short_idle,
                    })
                } else {
                    trainer_pool.update_state(&peer_id, |s| s.waiting_for_model = true);
                    ExecutorAction::Train(TrainAction::WaitForModel {
                        timeout: wait_model,
                    })
                }
            }
            RlTrainStatus::WaitedForModel => {
                let sending_peer = {
                    let snapshot = trainer_pool.info();
                    let worker = snapshot.iter().find(|w| w.peer_id == peer_id);
                    worker.and_then(|w| w.state.receiving_from)
                };

                if let Some(sending_peer) = sending_peer {
                    trainer_pool.update_state(&peer_id, |s| s.receiving_from = None);
                    ExecutorAction::RlTrain(RlTrainAction::ReceiveModel {
                        source: Reference::Peers {
                            peers: vec![sending_peer],
                            strategy: SelectionStrategy::All,
                            resource: None,
                        },
                        timeout: long_io,
                    })
                } else {
                    ExecutorAction::RlTrain(RlTrainAction::WaitForModel {
                        timeout: wait_model,
                    })
                }
            }
            RlTrainStatus::ReceivedModel => {
                // Lazy transition to other state
                ExecutorAction::RlTrain(RlTrainAction::Idle { timeout: now })
            }
            RlTrainStatus::SentModel => {
                // Lazy transition to other state
                ExecutorAction::RlTrain(RlTrainAction::Idle { timeout: now })
            }
            RlTrainStatus::Idle => {
                let mut state = round_state.lock().await;
                if !state.training_complete {
                    let snapshot = trainer_pool.info();
                    let peer_position = snapshot
                        .iter()
                        .position(|w| w.peer_id == peer_id)
                        .unwrap_or(0);

                    // Get peer contribution from pool
                    let peer_contribution = snapshot
                        .iter()
                        .find(|w| w.peer_id == peer_id)
                        .map(|w| w.state.samples_processed)
                        .unwrap_or(0);

                    let (count, update_target) = {
                        let training = training_state.lock().await;
                        (training.get_count(), training.get_update_target())
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

                    let (should_update, projected_target, batches) = if update_target <= count {
                        (true, count, 0)
                    } else if !snapshot.is_empty() && stats.iter().all(|&s| s > 0 && s < u64::MAX) {
                        let (time, cnt, projection, _) = S::project(
                            &progress,
                            &batch_sizes,
                            stats,
                            update_target.saturating_sub(count),
                            multi_batch_size,
                        );

                        tracing::debug!(
                            time = %time,
                            count = %cnt,
                            peer = %peer_id,
                            peer_position,
                            "Simulation with projection {:?} and {:?}",
                            projection,
                            update_target.saturating_sub(count)
                        );
                        (
                            false,
                            count.saturating_add(cnt.unsigned_abs()),
                            projection[peer_position],
                        )
                    } else {
                        (false, count, 1)
                    };

                    // Check if peer has applied update or sent update
                    let (has_applied_update, _applied_final_update) = snapshot
                        .iter()
                        .find(|w| w.peer_id == peer_id)
                        .map(|w| (w.state.applied_update, w.state.applied_final_update))
                        .unwrap_or((false, false));
                    let has_sent_update = primary_ps
                        .and_then(|ps| {
                            parameter_pool
                                .info()
                                .into_iter()
                                .find(|w| w.peer_id == ps)
                                .map(|w| w.worker_updates.contains(&peer_id))
                        })
                        .unwrap_or(false);

                    if state.aggregated_updates && !has_applied_update {
                        ExecutorAction::RlTrain(RlTrainAction::ApplyUpdate {
                            source: Reference::Peers {
                                peers: parameter_servers,
                                strategy: SelectionStrategy::All,
                                resource: None,
                            },
                            timeout: now + Duration::from_secs(10),
                        })
                    } else if has_sent_update {
                        ExecutorAction::RlTrain(RlTrainAction::Idle {
                            timeout: short_idle,
                        })
                    } else if !should_update {
                        ExecutorAction::RlTrain(RlTrainAction::ExecuteBatch { batches })
                    } else if parameter_servers.is_empty() {
                        // NOTE: If we need to send an update but there are no parameter servers,
                        // we must wait (idle) until one becomes available.
                        ExecutorAction::RlTrain(RlTrainAction::Idle {
                            timeout: short_idle,
                        })
                    } else {
                        ExecutorAction::RlTrain(RlTrainAction::SendUpdate {
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
                    ExecutorAction::RlTrain(RlTrainAction::Terminate)
                } else {
                    let snapshot = trainer_pool.info();
                    let has_push_assignment = snapshot.iter().any(|w| w.state.is_pusher);
                    let (applied_final_update, _am_pusher, has_applied_update) = snapshot
                        .iter()
                        .find(|w| w.peer_id == peer_id)
                        .map(|w| {
                            (
                                w.state.applied_final_update,
                                w.state.is_pusher,
                                w.state.applied_update,
                            )
                        })
                        .unwrap_or((false, false, false));

                    if state.aggregated_updates && !has_applied_update {
                        ExecutorAction::RlTrain(RlTrainAction::ApplyUpdate {
                            source: Reference::Peers {
                                peers: parameter_servers,
                                strategy: SelectionStrategy::All,
                                resource: None,
                            },
                            timeout: now + Duration::from_secs(10),
                        })
                    } else if !has_push_assignment && applied_final_update {
                        // Assign self as pusher
                        trainer_pool.update_state(&peer_id, |s| s.is_pusher = true);

                        if let Some(destination) = push_destination.as_ref().as_ref() {
                            ExecutorAction::RlTrain(RlTrainAction::PushToHub {
                                repository: destination.repository.clone(),
                                token: destination.token.clone(),
                            })
                        } else {
                            // Should not occur due to guard above.
                            state.push_done = true;
                            ExecutorAction::RlTrain(RlTrainAction::Terminate)
                        }
                    } else {
                        ExecutorAction::RlTrain(RlTrainAction::Idle {
                            timeout: short_idle,
                        })
                    }
                }
            }
            RlTrainStatus::BatchCompleted {
                batch_size,
                batches,
            } => {
                trainer_pool.update_statistics(&peer_id, |stats, last_updated| {
                    if *last_updated > 0 {
                        stats.update(since_start.saturating_sub(*last_updated), batches.into());
                    }
                    *last_updated = since_start;
                });

                let snapshot = trainer_pool.info();
                let peer_position = snapshot
                    .iter()
                    .position(|w| w.peer_id == peer_id)
                    .unwrap_or(0);

                let (count, update_target) = {
                    let mut training = training_state.lock().await;
                    training.record_batch(batch_size * batches);
                    (training.get_count(), training.get_update_target())
                };

                // Update per-worker samples
                trainer_pool.update_state(&peer_id, |s| {
                    s.samples_processed = s.samples_processed.saturating_add(batch_size * batches);
                });

                // Get fresh snapshot for peer contribution after update
                let peer_contribution = snapshot
                    .iter()
                    .find(|w| w.peer_id == peer_id)
                    .map(|w| w.state.samples_processed.saturating_add(batch_size))
                    .unwrap_or(batch_size);

                let (training_complete, sent_update) = {
                    let state = round_state.lock().await;
                    let sent = primary_ps
                        .and_then(|ps| {
                            parameter_pool
                                .info()
                                .into_iter()
                                .find(|w| w.peer_id == ps)
                                .map(|w| w.worker_updates.contains(&peer_id))
                        })
                        .unwrap_or(false);
                    (state.training_complete, sent)
                };

                if training_complete || sent_update {
                    ExecutorAction::RlTrain(RlTrainAction::Idle {
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

                    let (should_update, projected_target, batches) = if update_target <= count {
                        (true, count, 0)
                    } else if !snapshot.is_empty() && stats.iter().all(|&s| s > 0 && s < u64::MAX) {
                        let (time, cnt, projection, _) = S::project(
                            &progress,
                            &batch_sizes,
                            stats,
                            update_target.saturating_sub(count),
                            multi_batch_size,
                        );

                        tracing::debug!(
                            time = %time,
                            count = %cnt,
                            peer = %peer_id,
                            peer_position,
                            "Simulation with projection {:?} and {:?}",
                            projection,
                            update_target.saturating_sub(count)
                        );
                        (
                            false,
                            count.saturating_add(cnt.unsigned_abs()),
                            projection[peer_position],
                        )
                    } else {
                        (false, count, 1)
                    };

                    if !should_update {
                        ExecutorAction::RlTrain(RlTrainAction::ExecuteBatch { batches })
                    } else if parameter_servers.is_empty() {
                        // NOTE: If we need to send an update but there are no parameter servers,
                        // we must wait (idle) until one becomes available.
                        ExecutorAction::RlTrain(RlTrainAction::Idle {
                            timeout: short_idle,
                        })
                    } else {
                        ExecutorAction::RlTrain(RlTrainAction::SendUpdate {
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
            RlTrainStatus::SentUpdate { mut metrics, .. } => {
                let snapshot = trainer_pool.info();
                let worker_samples = snapshot
                    .iter()
                    .find(|w| w.peer_id == peer_id)
                    .map(|w| w.state.samples_processed)
                    .unwrap_or(0) as f32;

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
                metrics.insert("data_points".to_string(), worker_samples);
                metrics.insert("steps".to_string(), worker_steps_per_sec);
                metrics.insert("duration".to_string(), elapsed_secs);

                tx.send((peer_id, Metrics { round, metrics }))
                    .await
                    .map_err(RLSchedulerError::from)?;

                // NOTE: Track workers that have sent their update for the current round on the parameter pool.
                if let Some(ps) = primary_ps {
                    parameter_pool.update_state(&ps, |state| {
                        state.worker_updates.insert(peer_id);
                    });
                }
                let snapshot = trainer_pool.info(); // Refresh after update for logging

                let mut state = round_state.lock().await;
                if state.first_update_at.is_none() {
                    state.first_update_at = Some(Instant::now());
                }
                let total_workers = snapshot.len();
                let sent = primary_ps
                    .and_then(|ps| {
                        parameter_pool
                            .info()
                            .into_iter()
                            .find(|w| w.peer_id == ps)
                            .map(|w| w.worker_updates.len())
                    })
                    .unwrap_or(0);
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

                ExecutorAction::RlTrain(RlTrainAction::Idle {
                    timeout: short_idle,
                })
            }
            RlTrainStatus::AppliedUpdate => {
                let training_complete = {
                    trainer_pool.update_state(&peer_id, |s| s.applied_update = true);

                    // Reset the `last_update` counter to factor out the update time.
                    // Otherwise it will bias the runtim statistics, e.g. the update time
                    // will introduce a larger bias for single batches than for mulit-batches
                    trainer_pool.update_statistics(&peer_id, |_, last_updated| {
                        *last_updated = since_start;
                    });

                    let state = round_state.lock().await;
                    if state.training_complete {
                        trainer_pool.update_state(&peer_id, |s| s.applied_final_update = true);
                        true
                    } else {
                        false
                    }
                };

                if training_complete {
                    ExecutorAction::RlTrain(RlTrainAction::Idle {
                        timeout: now + Duration::from_millis(500),
                    })
                } else {
                    // Find a worker waiting for model
                    let snapshot = trainer_pool.info();
                    let waiting_worker = snapshot
                        .iter()
                        .find(|w| w.state.waiting_for_model)
                        .map(|w| w.peer_id);

                    if let Some(update_worker) = waiting_worker {
                        // Mark them as not waiting and set receiving from
                        trainer_pool.update_state(&update_worker, |s| {
                            s.waiting_for_model = false;
                            s.receiving_from = Some(peer_id);
                        });

                        ExecutorAction::RlTrain(RlTrainAction::SendModel {
                            target: Reference::Peers {
                                peers: vec![update_worker],
                                strategy: SelectionStrategy::One,
                                resource: None,
                            },
                        })
                    } else {
                        ExecutorAction::RlTrain(RlTrainAction::ExecuteBatch { batches: 1 })
                    }
                }
            }
            RlTrainStatus::PushedToHub => {
                let is_pusher = trainer_pool
                    .info()
                    .iter()
                    .find(|w| w.peer_id == peer_id)
                    .map(|w| w.state.is_pusher)
                    .unwrap_or(false);

                if is_pusher {
                    let mut state = round_state.lock().await;
                    state.push_done = true;
                }

                ExecutorAction::RlTrain(RlTrainAction::Terminate)
            }
            RlTrainStatus::Error(TrainError::Connection { message }) => {
                tracing::warn!(%peer_id, message = %message, "Worker reported connection error");

                trainer_pool.update_state(&peer_id, |s| {
                    if s.is_pusher {
                        s.is_pusher = false;
                    }
                });

                ExecutorAction::RlTrain(RlTrainAction::Idle {
                    timeout: short_idle,
                })
            }
            RlTrainStatus::Error(TrainError::Other { message }) => {
                tracing::warn!(%peer_id, message = %message, "Worker reported error");
                trainer_pool.update_state(&peer_id, |s| {
                    if s.is_pusher {
                        s.is_pusher = false;
                    }
                });
                ExecutorAction::RlTrain(RlTrainAction::Terminate)
            }
            RlTrainStatus::Terminated => ExecutorAction::RlTrain(RlTrainAction::Terminate),
        },
        ExecutorStatus::Aggregate(aggregate_status) => match aggregate_status {
            AggregateStatus::Idle => {
                let training_complete = { round_state.lock().await.training_complete };
                if training_complete {
                    let all_applied = trainer_pool
                        .info()
                        .iter()
                        .all(|w| w.state.applied_final_update);

                    if all_applied {
                        ExecutorAction::Aggregate(AggregateAction::Terminate)
                    } else {
                        ExecutorAction::Aggregate(AggregateAction::Idle {
                            timeout: short_idle,
                        })
                    }
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
                    let snapshot = trainer_pool.info();
                    let workers: Vec<_> = snapshot
                        .iter()
                        .filter(|w| !w.state.waiting_for_model)
                        .map(|w| w.peer_id)
                        .collect();

                    if workers.is_empty() {
                        ExecutorAction::Aggregate(AggregateAction::Idle {
                            timeout: short_idle,
                        })
                    } else {
                        // Start aggregation when either all workers have sent updates,
                        // or when a quorum (min workers) have sent updates and the
                        // grace period has elapsed since the first update in this round.
                        let mut state = round_state.lock().await;
                        let aggregate_snapshot = parameter_pool.info();
                        let ps_state = aggregate_snapshot.iter().find(|ps| ps.peer_id == peer_id);
                        let all_sent = ps_state
                            .map(|entry| workers.iter().all(|w| entry.worker_updates.contains(w)))
                            .unwrap_or(false);

                        let sent_count = ps_state
                            .map(|entry| entry.worker_updates.len())
                            .unwrap_or(0);

                        let effective_quorum = state.min_quorum.min(workers.len());
                        let quorum_met = sent_count >= effective_quorum;
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
                    let workers: Vec<_> =
                        trainer_pool.info().into_iter().map(|w| w.peer_id).collect();

                    if workers.is_empty() {
                        ExecutorAction::Aggregate(AggregateAction::Idle {
                            timeout: short_idle,
                        })
                    } else {
                        // Log that we are moving to broadcast for this round.
                        let round = {
                            let mut state = round_state.lock().await;
                            state.aggregated_updates = true;
                            // reset applied updates in pool
                            let snapshot = trainer_pool.info();
                            for w in snapshot {
                                trainer_pool.update_state(&w.peer_id, |s| s.applied_update = false);
                            }
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
                .map_err(RLSchedulerError::from)?;

                let next_round = {
                    let mut state = round_state.lock().await;

                    tracing::info!(round = state.round, "Broadcast completed; advancing round");

                    // Reset per-round state in pool
                    let snapshot = trainer_pool.info();
                    for w in snapshot {
                        trainer_pool.update_state(&w.peer_id, |s| {
                            s.sent_update = false;
                            s.samples_processed = 0;
                        });
                    }
                    if let Some(ps) = primary_ps {
                        parameter_pool.update_state(&ps, |state| state.worker_updates.clear());
                    }

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

                {
                    let mut state = round_state.lock().await;
                    if !state.training_complete {
                        state.aggregated_updates = false;
                    }
                }
                ExecutorAction::Aggregate(AggregateAction::Idle {
                    timeout: short_idle,
                })
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

pub struct RLScheduler {}

impl RLScheduler {
    pub async fn run<T, S>(
        network: Network,
        gymnasium_pool: PoolWithTrainInfoHandle<T>,
        trainer_pool: PoolWithTrainInfoHandle<T>,
        parameter_pool: PoolWithAggregateInfoHandle,
        id: Uuid,
        min_quorum: usize,
        grace: Duration,
        samples_between_updates: u32,
        update_rounds: u32,
        push_destination: Option<ModelDestination>,
        batch_sizer: BatchSizer,
        multi_batch_size: u32,
        cancel: CancellationToken,
    ) -> Result<(mpsc::Receiver<(PeerId, Metrics)>, JoinHandle<()>), RLSchedulerError>
    where
        T: RuntimeStatistic + 'static,
        S: Simulation + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::channel(100);
        let start = std::time::Instant::now();
        let push_destination = Arc::new(push_destination);
        let stream_handle = tokio::spawn({
            let round_state = Arc::new(Mutex::new(RoundState {
                first_update_at: None,
                round_started_at: start,
                aggregate_started_at: None,
                min_quorum,
                grace,
                round: 0,
                update_rounds,
                training_complete: false,
                push_done: false,
                aggregated_updates: false,
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
                .map_err(RLSchedulerError::from)?
                .respond_with_concurrent(None, move |request| {
                    let tx = tx.clone();
                    let gymnasium_pool = gymnasium_pool.clone();
                    let trainer_pool = trainer_pool.clone();
                    let parameter_pool = parameter_pool.clone();
                    let round_state = round_state.clone();
                    let training_state = training_state.clone();
                    let batch_sizer = batch_sizer.clone();
                    let push_destination = push_destination.clone();
                    let cancel = cancel.clone();
                    async move {
                        match schedule::<T, S>(
                            tx,
                            gymnasium_pool,
                            trainer_pool,
                            parameter_pool,
                            round_state,
                            training_state,
                            batch_sizer,
                            multi_batch_size,
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
                                    next: ExecutorAction::Gymnasium(GymnasiumAction::Terminate),
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
