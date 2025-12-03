//! Pool-based worker management with pull-based routing.
//!
//! The pool maintains a set of workers, automatically replacing failed ones
//! and pursuing a target size.

use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    fmt::Display,
    future::Future,
    pin::Pin,
    sync::{Arc, RwLock},
    task::{Context, Poll},
    time::{Duration, Instant, SystemTime},
};

use arc_swap::ArcSwap;
use futures_util::{Stream, StreamExt, stream::FuturesUnordered};
use hypha_messages::WorkerSpec;
use hypha_resources::Resources;
use libp2p::PeerId;
use tokio::{
    sync::mpsc,
    task::JoinHandle,
    time::{MissedTickBehavior, interval},
};

use crate::{
    allocator::{Allocator, AllocatorError, DEFAULT_TIMEOUT},
    scheduler_config::PriceRange,
    statistics::RuntimeStatistic,
    worker::{Worker, WorkerError},
};

/// Lightweight descriptor containing worker information.
///
/// This is what consumers receive when iterating over the pool.
/// The pool maintains the worker lifecycle internally.
#[derive(Clone, Debug)]
pub struct WorkerDescriptor {
    pub peer_id: PeerId,
    pub resources: Resources,
    /// Join timestamp used for deterministic ordering of pool members.
    ///
    /// NOTE: We keep this to ensure stable, oldest-first ordering of the
    /// member list exposed via `Pool::members()`. This prevents incidental
    /// reordering due to allocation batch order or concurrent joins.
    pub joined_at: SystemTime,
}

impl Display for WorkerDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.peer_id)
    }
}

/// A cloneable handle for querying pool membership.
///
/// This allows querying current members without holding any lock on the Pool itself,
/// enabling concurrent streaming (Pool::next) and membership queries.
#[derive(Clone)]
pub struct PoolMembers {
    inner: Arc<ArcSwap<Vec<WorkerDescriptor>>>,
}

pub struct PoolMemberIter {
    inner: Arc<Vec<WorkerDescriptor>>,
    index: usize,
}

impl Iterator for PoolMemberIter {
    type Item = WorkerDescriptor;

    fn next(&mut self) -> Option<Self::Item> {
        let worker = self.inner.get(self.index)?.clone();
        self.index += 1;
        Some(worker)
    }
}

impl PoolMembers {
    pub fn iter(&self) -> PoolMemberIter {
        PoolMemberIter {
            inner: self.inner.load_full(),
            index: 0,
        }
    }

    /// Returns the current number of active workers.
    pub fn len(&self) -> usize {
        self.inner.load().len()
    }

    /// Returns true if the pool has no active workers.
    pub fn is_empty(&self) -> bool {
        self.inner.load().is_empty()
    }
}

impl From<&Worker> for WorkerDescriptor {
    fn from(worker: &Worker) -> Self {
        Self {
            peer_id: worker.peer_id(),
            resources: *worker.resources(),
            // NOTE: Use the time of insertion into the pool as the join time
            // to provide a stable ordering for routing decisions that rely
            // on picking the "oldest" member.
            joined_at: SystemTime::now(),
        }
    }
}

/// Pool configuration.
#[derive(Clone, Debug)]
pub struct PoolConfig {
    /// Human-readable name for logging.
    pub name: String,
    /// Worker specification for allocation requests.
    pub spec: WorkerSpec,
    /// Acceptable price range.
    pub price: PriceRange,
    /// Minimum workers required. Job aborts if below this after grace period.
    pub min: usize,
    /// Target workers to pursue opportunistically.
    pub target: usize,
    /// Grace period when below min before aborting.
    pub grace: Duration,
}

/// Error indicating pool failure.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("pool '{name}' remained below minimum {min} workers after grace period expired")]
    GraceExpired { name: String, min: usize },
    #[error("pool task failed to join")]
    Join(#[source] tokio::task::JoinError),
}

/// A pool of workers that maintains membership and yields new workers as they join.
///
/// # Usage
///
/// ```ignore
/// let mut pool = Pool::new(allocator, config);
///
/// // Wait for minimum workers
/// // Wait for minimum workers using an external wait loop or grace logic
///
/// // Dispatch jobs to workers as they join
/// while let Some(result) = pool.next().await {
///     let worker = result?;
///     dispatch_job(&worker).await;
/// }
///
/// // Query current membership for routing (anytime)
/// let peers: Vec<PeerId> = pool.members().iter().map(|w| w.peer_id).collect();
/// ```
pub struct Pool {
    /// Shared membership state - the single source of truth.
    members: Arc<ArcSwap<Vec<WorkerDescriptor>>>,

    /// Receives new workers from the background task.
    joins_rx: mpsc::Receiver<WorkerDescriptor>,

    /// Background task handle.
    task: Option<JoinHandle<Result<(), PoolError>>>,
}

impl Pool {
    /// Create a new pool with the given allocator and configuration.
    pub fn new<A>(allocator: A, config: PoolConfig) -> Self
    where
        A: Allocator + 'static,
    {
        let members = Arc::new(ArcSwap::from_pointee(Vec::new()));
        let (joins_tx, joins_rx) = mpsc::channel(64);

        let handle = tokio::spawn(run_pool(allocator, config, Arc::clone(&members), joins_tx));

        Self {
            members,
            joins_rx,
            task: Some(handle),
        }
    }

    /// Returns a cloneable handle for querying membership.
    ///
    /// Use this when you need to query membership from multiple tasks concurrently
    /// while still consuming the Pool stream (via `next()`).
    pub fn members(&self) -> PoolMembers {
        PoolMembers {
            inner: Arc::clone(&self.members),
        }
    }

    /// Returns a handle for read-only access to membership/dispatchers.
    pub fn handle(&self) -> PoolHandle {
        PoolHandle {
            members: self.members(),
        }
    }
}

impl Stream for Pool {
    type Item = Result<WorkerDescriptor, PoolError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Check background task for completion and error
        if let Some(task) = self.task.as_mut() {
            match Pin::new(task).poll(cx) {
                Poll::Ready(Ok(Ok(()))) => {
                    self.task = None;
                }
                Poll::Ready(Ok(Err(err))) => {
                    self.task = None;

                    return Poll::Ready(Some(Err(err)));
                }
                Poll::Ready(Err(join_err)) => {
                    self.task = None;

                    return Poll::Ready(Some(Err(PoolError::Join(join_err))));
                }
                Poll::Pending => {}
            }
        }

        // Check for new workers
        match Pin::new(&mut self.joins_rx).poll_recv(cx) {
            Poll::Ready(Some(worker)) => Poll::Ready(Some(Ok(worker))),
            Poll::Ready(None) => Poll::Ready(None), // Pool shut down
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Read-only view over pool membership and dispatchers.
#[derive(Clone)]
pub struct PoolHandle {
    members: PoolMembers,
}

impl PoolHandle {
    pub fn members(&self) -> PoolMembers {
        self.members.clone()
    }
}

/// Internal state for grace period tracking.
async fn run_pool<A>(
    allocator: A,
    config: PoolConfig,
    members: Arc<ArcSwap<Vec<WorkerDescriptor>>>,
    joins_tx: mpsc::Sender<WorkerDescriptor>,
) -> Result<(), PoolError>
where
    A: Allocator,
{
    let PoolConfig {
        name,
        spec,
        price,
        min,
        target,
        grace,
    } = config;
    // Track worker exit futures
    #[allow(clippy::type_complexity)]
    let mut watchers: FuturesUnordered<
        Pin<Box<dyn Future<Output = (PeerId, Result<(), WorkerError>)> + Send>>,
    > = FuturesUnordered::new();

    // Reconciliation ticker
    let mut ticker = interval(Duration::from_millis(50));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let mut below_min_since = None;

    loop {
        tokio::select! {
            // Periodic reconciliation
            _ = ticker.tick() => {
                let current_count = members.load().len();

                if current_count < min {
                    match below_min_since {
                        Some(start) => {
                            if start + grace <= Instant::now() {
                                let err = PoolError::GraceExpired { name: name.clone(), min };

                                return Err(err);
                            }
                        }
                        None => {
                            tracing::warn!(pool = %name, count = current_count, %min,
                                "Below minimum number of workers");

                            below_min_since = Some(Instant::now());
                        }
                    }
                } else if below_min_since.is_some() {
                    tracing::debug!(
                        pool = %name,
                        count = current_count,
                        min = %min,
                        target = %target,
                        "Recovered to {current_count} workers");

                    below_min_since = None;
                }

                // Try to allocate more workers if below target
                if current_count < target {
                    let needed = target - current_count;
                    tracing::debug!(
                        pool = %name,
                        count = current_count,
                        min = %min,
                        target = %target,
                        "Reconciling pool");

                    match allocator
                        .request(spec.clone(), price, Some(DEFAULT_TIMEOUT), needed)
                        .await
                    {
                        Ok(workers) => {
                            tracing::debug!(pool = %name, "Allocated {} workers", workers.len());

                            if !workers.is_empty() {
                                // NOTE: Members are stored in an ArcSwap so we need to first get a
                                // snapshot, then update that snapshot and then store the update. Do
                                // this once per allocation batch instead of per worker.
                                let mut members_snapshot = members.load().as_ref().clone();

                                let mut descriptors = Vec::with_capacity(workers.len());

                                for worker in workers {
                                    let descriptor = WorkerDescriptor::from(&worker);
                                    let peer_id = descriptor.peer_id;

                                    members_snapshot.push(descriptor.clone());
                                    descriptors.push(descriptor);

                                    // NOTE: We need to keep the peer id along with the worker handle to identify the worker when the handle completes.
                                    watchers.push(Box::pin(async move {
                                        let result = worker.await;

                                        (peer_id, result)
                                    }));
                                }

                                // NOTE: Ensure deterministic, oldest-first ordering by join time.
                                // This keeps routing decisions stable across concurrent joins
                                // and allocator return order.
                                //
                                // QUESTION: Should we consider a tiebreaker? `SystemTime` has
                                // sufficient granularity here and is assigned per insertion, so
                                // ties are highly unlikely. If needed we could add a peer_id
                                // secondary key.
                                // NOTE: Ensure deterministic, oldest-first ordering by join time
                                // with a `peer_id` tiebreaker for strict total ordering.
                                members_snapshot.sort_by(|a, b| {
                                    a.joined_at
                                        .cmp(&b.joined_at)
                                        .then_with(|| a.peer_id.to_bytes().cmp(&b.peer_id.to_bytes()))
                                });

                                // NOTE: Store updated membership before emitting descriptors on
                                // the stream to maintain consistency.
                                members.store(Arc::new(members_snapshot));

                                for descriptor in descriptors {
                                    tracing::debug!(pool = %name, worker=%descriptor,
                                        "Worker joined");

                                    let _ = joins_tx.send(descriptor).await;

                                }
                            }
                        }
                        Err(AllocatorError::NoOffersReceived | AllocatorError::Timeout) => {
                            tracing::debug!(pool = %name, needed, "No offers while reconciling");
                        }
                        Err(err) => {
                            tracing::warn!(pool = %name, ?err, "Allocation failed");
                        }
                    }
                }
            }

            // Worker exit
            Some((peer_id, exit_result)) = watchers.next() => {
                // NOTE:Remove from membership
                let mut members_snapshot = members.load().as_ref().clone();
                members_snapshot.retain(|w| w.peer_id != peer_id);
                members.store(Arc::new(members_snapshot));

                if let Err(e) = exit_result {
                    tracing::warn!(pool = %name, %peer_id, error = ?e, "Worker exited with error");
                }
            }

            else => break,
        }
    }

    Ok(())
}

/// Descriptor enriched with an optional statistic value.
#[derive(Clone, Debug)]
pub struct WorkerDescriptorWithStats {
    pub peer_id: PeerId,
    pub resources: Resources,
    pub last_updated: Option<LastUpdated>,
    pub statistic: Option<u64>,
}

impl WorkerDescriptorWithStats {
    pub fn new(
        descriptor: &WorkerDescriptor,
        last_updated: Option<LastUpdated>,
        statistic: Option<u64>,
    ) -> Self {
        Self {
            peer_id: descriptor.peer_id,
            resources: descriptor.resources,
            last_updated,
            statistic,
        }
    }
}

/// Snapshot view of current members enriched with statistics.
type LastUpdated = u64;

pub struct PoolWithStatistics<T: RuntimeStatistic> {
    pool: Pool,
    // TODO: Consider using DashMap instead of RwLock<HashMap> if performance becomes an issue
    statistics: Arc<RwLock<HashMap<PeerId, (LastUpdated, T)>>>,
}

impl<T> PoolWithStatistics<T>
where
    T: RuntimeStatistic,
{
    pub fn new(pool: Pool) -> Self {
        Self {
            pool,
            statistics: Arc::new(RwLock::new(HashMap::default())),
        }
    }

    /// Returns a cloneable handle that exposes statistics and membership without owning the stream.
    pub fn handle(&self) -> PoolStatisticsHandle<T> {
        PoolStatisticsHandle {
            pool: self.pool.handle(),
            statistics: Arc::clone(&self.statistics),
            _marker: std::marker::PhantomData,
        }
    }

    /// Returns current members decorated with their latest statistic (if any),
    /// after pruning stale statistics.
    ///
    /// NOTE: For consistency it snapshots both the members list and the pruned statistics.
    pub fn statistics(&self) -> Vec<WorkerDescriptorWithStats> {
        let members = self.pool.members();

        let snapshot = members.inner.load_full();
        let active: HashSet<PeerId> = snapshot.iter().map(|worker| worker.peer_id).collect();

        // Prune stale statistics
        let mut statistics = self.statistics.write().expect("statistics lock poisoned");
        statistics.retain(|peer_id, _| active.contains(peer_id));

        let statistics_snapshot: HashMap<PeerId, (LastUpdated, u64)> = statistics
            .iter()
            .map(|(peer_id, (last_updated, stats))| (*peer_id, (*last_updated, stats.value())))
            .collect();

        snapshot
            .iter()
            .map(|descriptor| {
                let statistic = statistics_snapshot.get(&descriptor.peer_id).copied();

                WorkerDescriptorWithStats::new(
                    descriptor,
                    statistic.map(|(last_updated, _)| last_updated),
                    statistic.map(|(_, stat)| stat),
                )
            })
            .collect()
    }

    /// Update statistics for a worker with the given timestamp.
    pub fn update(&self, peer_id: &PeerId, now: u64) {
        let mut statistics = self.statistics.write().expect("statistics lock poisoned");

        match statistics.entry(*peer_id) {
            Entry::Occupied(mut entry) => {
                let (last_updated, stats) = entry.get_mut();
                stats.update(now.saturating_sub(*last_updated));
                *last_updated = now;
            }
            Entry::Vacant(entry) => {
                entry.insert((now, T::default()));
            }
        }
    }
}

impl<T> Stream for PoolWithStatistics<T>
where
    T: RuntimeStatistic,
{
    type Item = Result<WorkerDescriptor, PoolError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.pool).poll_next(cx)
    }
}

impl<T> PoolWithStatistics<T> where T: RuntimeStatistic {}

/// Cloneable handle for statistics + membership/dispatchers without owning the stream.
pub struct PoolStatisticsHandle<T: RuntimeStatistic> {
    pool: PoolHandle,
    statistics: Arc<RwLock<HashMap<PeerId, (LastUpdated, T)>>>,
    _marker: std::marker::PhantomData<T>,
}

impl<T: RuntimeStatistic> Clone for PoolStatisticsHandle<T> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            statistics: Arc::clone(&self.statistics),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<T> PoolStatisticsHandle<T>
where
    T: RuntimeStatistic,
{
    pub fn statistics(&self) -> Vec<WorkerDescriptorWithStats> {
        let members = self.pool.members();

        let snapshot = members.inner.load_full();
        let active: HashSet<PeerId> = snapshot.iter().map(|worker| worker.peer_id).collect();

        let mut statistics = self.statistics.write().expect("statistics lock poisoned");
        statistics.retain(|peer_id, _| active.contains(peer_id));

        let statistics_snapshot: HashMap<PeerId, (LastUpdated, u64)> = statistics
            .iter()
            .map(|(peer_id, (last_updated, stats))| (*peer_id, (*last_updated, stats.value())))
            .collect();

        snapshot
            .iter()
            .map(|descriptor| {
                let statistic = statistics_snapshot.get(&descriptor.peer_id).copied();

                WorkerDescriptorWithStats::new(
                    descriptor,
                    statistic.map(|(last_updated, _)| last_updated),
                    statistic.map(|(_, stat)| stat),
                )
            })
            .collect()
    }

    pub fn update(&self, peer_id: &PeerId, now: u64) {
        let mut statistics = self.statistics.write().expect("statistics lock poisoned");

        match statistics.entry(*peer_id) {
            Entry::Occupied(mut entry) => {
                let (last_updated, stats) = entry.get_mut();
                stats.update(now.saturating_sub(*last_updated));
                *last_updated = now;
            }
            Entry::Vacant(entry) => {
                entry.insert((now, T::default()));
            }
        }
    }

    pub fn members(&self) -> PoolMembers {
        self.pool.members()
    }
}

#[cfg(test)]
mod tests {
    use hypha_resources::Resources;
    use tokio::time::{Duration, sleep, timeout};

    use super::*;
    use crate::{statistics::RunningMean, worker::TestWorkerBuilder};

    struct StubAllocator {
        responses: tokio::sync::Mutex<Vec<Vec<Worker>>>,
    }

    impl StubAllocator {
        fn new(responses: Vec<Vec<Worker>>) -> Self {
            Self {
                responses: tokio::sync::Mutex::new(responses),
            }
        }
    }

    impl Allocator for StubAllocator {
        fn request(
            &self,
            _spec: WorkerSpec,
            _price: PriceRange,
            _deadline: Option<Duration>,
            num: usize,
        ) -> impl std::future::Future<Output = Result<Vec<Worker>, AllocatorError>> + Send {
            async move {
                let mut responses = self.responses.lock().await;
                if let Some(batch) = responses.pop() {
                    let workers: Vec<Worker> = batch.into_iter().take(num).collect();
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

    /// Create a test worker that stays alive until the test ends.
    fn test_worker() -> Worker {
        let lease_handler = tokio::spawn(async move {
            // Wait forever (until the task is aborted)
            futures_util::future::pending::<()>().await;
            Ok(())
        });
        TestWorkerBuilder::new()
            .with_lease_handler(lease_handler)
            .build()
    }

    fn test_config(min: usize, target: usize) -> PoolConfig {
        PoolConfig {
            name: "test".into(),
            spec: WorkerSpec {
                resources: Resources::default(),
                executor: Vec::new(),
            },
            price: PriceRange::default(),
            min,
            target,
            grace: Duration::from_millis(200),
        }
    }

    /// Create a test worker whose lease fails after a given delay.
    fn failing_worker(error: WorkerError, fail_after: Duration) -> Worker {
        let lease_handler = tokio::spawn(async move {
            sleep(fail_after).await;
            Err(error)
        });
        TestWorkerBuilder::new()
            .with_lease_handler(lease_handler)
            .build()
    }

    #[tokio::test]
    async fn pool_ready_when_min_reached() {
        let w1 = test_worker();
        let w2 = test_worker();
        let allocator = StubAllocator::new(vec![vec![w1, w2]]);

        let mut pool = Pool::new(allocator, test_config(2, 2));

        let result = timeout(Duration::from_secs(2), async {
            let mut count = 0;
            while let Some(res) = pool.next().await {
                res.unwrap();
                count = pool.members().len();
                if count >= 2 {
                    break;
                }
            }
            count
        })
        .await;
        assert!(result.is_ok(), "Should complete");
        assert_eq!(result.unwrap(), 2);
    }

    #[tokio::test]
    async fn pool_fails_grace_when_min_not_reached() {
        let allocator = StubAllocator::new(vec![]); // No workers available

        let mut pool = Pool::new(
            allocator,
            PoolConfig {
                grace: Duration::from_millis(100),
                ..test_config(2, 2)
            },
        );

        let result = timeout(Duration::from_secs(2), async {
            loop {
                match pool.next().await {
                    Some(Err(err)) => break err,
                    Some(Ok(_)) => {}
                    None => panic!("pool ended without fatal error"),
                }
            }
        })
        .await;
        assert!(result.is_ok(), "Should complete");
        assert!(matches!(result.unwrap(), PoolError::GraceExpired { .. }));
    }

    #[tokio::test]
    async fn pool_yields_workers_via_stream() {
        let w1 = test_worker();
        let w2 = test_worker();
        let allocator = StubAllocator::new(vec![vec![w1, w2]]);

        let mut pool = Pool::new(allocator, test_config(2, 2));
        // Workers should be available via stream
        let mut count = 0;
        while let Ok(Some(result)) = timeout(Duration::from_millis(100), pool.next()).await {
            assert!(result.is_ok());
            count += 1;
            if count >= 2 {
                break;
            }
        }
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn members_updated_on_worker_exit() {
        let w1 = test_worker();
        let peer_id = w1.peer_id();
        let allocator = StubAllocator::new(vec![vec![w1]]);

        let mut pool = Pool::new(
            allocator,
            PoolConfig {
                min: 1,
                target: 1,
                grace: Duration::from_secs(10), // Long grace so we don't fail
                ..test_config(1, 1)
            },
        );

        // Wait for worker to join
        timeout(Duration::from_millis(200), async {
            while pool.members().is_empty() {
                if pool.next().await.is_none() {
                    break;
                }
            }
        })
        .await
        .unwrap();

        assert_eq!(pool.members().len(), 1);
        let first = pool.members().iter().next().expect("expected one member");
        assert_eq!(first.peer_id, peer_id);
    }

    #[tokio::test]
    async fn pool_enters_grace_after_lease_failure() {
        let worker = failing_worker(WorkerError::LeaseExpired, Duration::from_millis(100));
        let allocator = StubAllocator::new(vec![vec![worker]]);

        let mut pool = Pool::new(
            allocator,
            PoolConfig {
                grace: Duration::from_millis(50),
                ..test_config(1, 1)
            },
        );

        // Wait for the failing worker to be removed from membership
        timeout(Duration::from_millis(500), async {
            loop {
                if pool.members().is_empty() {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("worker should be removed after lease failure");

        // Expect the pool to surface GraceExpired once the grace window elapses
        let err = timeout(Duration::from_secs(1), async {
            loop {
                match pool.next().await {
                    Some(Err(err)) => break err,
                    Some(Ok(_)) => {}
                    None => panic!("pool ended without fatal error"),
                }
            }
        })
        .await
        .expect("pool should emit fatal error after grace expires");

        assert!(matches!(err, PoolError::GraceExpired { .. }));
    }

    #[tokio::test]
    async fn pool_with_stats_updates_and_prunes() {
        let worker = failing_worker(WorkerError::LeaseExpired, Duration::from_millis(30));
        let peer_id = worker.peer_id();
        let allocator = StubAllocator::new(vec![vec![worker]]);

        let mut pool_with_stats = PoolWithStatistics::<RunningMean>::new(Pool::new(
            allocator,
            PoolConfig {
                grace: Duration::from_millis(200),
                ..test_config(0, 1)
            },
        ));

        // Wait for the worker to join so we can update stats.
        timeout(Duration::from_millis(200), async {
            while let Some(res) = pool_with_stats.next().await {
                if res.is_ok() {
                    break;
                }
            }
        })
        .await
        .expect("worker should join before timeout");

        pool_with_stats.update(&peer_id, 10);
        pool_with_stats.update(&peer_id, 25);

        let statistics = pool_with_stats.statistics();
        let first = statistics.first().expect("expected member with stats");
        assert_eq!(first.peer_id, peer_id);
        assert_eq!(first.statistic, Some(15));

        // Wait for worker removal and ensure stale stats are pruned.
        timeout(Duration::from_millis(500), async {
            loop {
                if pool_with_stats.pool.members().is_empty() {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("worker should be removed");

        assert!(pool_with_stats.statistics().is_empty());
    }
}
