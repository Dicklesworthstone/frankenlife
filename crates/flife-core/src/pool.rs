//! Cached private worker pools enforcing FrankenLife's single worker budget
//! (plan §3.4, §12.5).
//!
//! Replacing per-phase `std::thread::scope` calls with cached private
//! [`rayon::ThreadPool`] instances eliminates hundreds of OS pthread spawns
//! (`bsdthread_create` / `mmap` / `munmap`) per tick while preserving
//! bit-exact deterministic work partitioning:
//!
//! - **Exact worker budget:** A call requesting `workers` total threads runs
//!   the caller inline via [`rayon::ThreadPool::in_place_scope`] and caches a
//!   private pool of `workers - 1` background threads (`0` threads when
//!   `workers <= 1`), so active OS threads never exceed `workers`.
//! - **Private pools only:** Never initializes or touches Rayon's global
//!   thread pool, avoiding nested pool contention with upstream libraries.
//! - **Lock-free fast path:** Worker counts `2..=64` resolve through a static
//!   [`OnceLock`] table with a single atomic acquire load (zero mutex, zero
//!   `Arc` refcount traffic per tick); larger counts fall back to a cached
//!   `BTreeMap`.
//! - **Non-nesting inline fallback:** Accidental nested calls from inside a
//!   `flife_core::pool` scope or worker thread immediately fall back to serial
//!   inline execution (never deadlocking or oversubscribing the worker budget).
//! - **Panic isolation:** Panics in either the calling thread or any spawned
//!   worker are caught at the scope boundary and mapped cleanly to
//!   [`CoreError::StateConflict`], leaving the cached pool healthy for
//!   subsequent calls.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::ops::Deref;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::error::CoreError;

thread_local! {
    /// Set once at startup on every cached private pool worker thread so
    /// accidental nested pool dispatch always falls back inline.
    static ON_POOL_THREAD: Cell<bool> = const { Cell::new(false) };
    /// Set on the calling thread while inside an active [`scope_workers`]
    /// parallel fanout, preventing nested fanout from the caller side too.
    static IN_SCOPE_DISPATCH: Cell<bool> = const { Cell::new(false) };
}

#[inline]
fn in_worker_dispatch() -> bool {
    ON_POOL_THREAD.get() || IN_SCOPE_DISPATCH.get()
}

/// RAII guard marking the calling thread as inside an active parallel scope
/// and restoring the flag on normal exit or panic unwind.
struct DispatchGuard;

impl DispatchGuard {
    #[inline]
    fn enter() -> Self {
        IN_SCOPE_DISPATCH.set(true);
        Self
    }
}

impl Drop for DispatchGuard {
    #[inline]
    fn drop(&mut self) {
        IN_SCOPE_DISPATCH.set(false);
    }
}

/// Highest worker count served from the lock-free [`OnceLock`] fast table.
const MAX_FAST_WORKERS: usize = 64;

/// Lock-free cache for private pools serving `workers = 2..=MAX_FAST_WORKERS`
/// (index `workers - 2` holds a pool with `workers - 1` background threads).
static FAST_POOLS: [OnceLock<Arc<rayon::ThreadPool>>; MAX_FAST_WORKERS - 1] =
    [const { OnceLock::new() }; MAX_FAST_WORKERS - 1];

/// Cold mutex serializing first-time creation of a [`FAST_POOLS`] slot so
/// concurrent callers never build duplicate pools for the same worker count.
static FAST_INIT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Fallback cache for rare worker budgets above [`MAX_FAST_WORKERS`].
static OVERFLOW_POOLS: OnceLock<Mutex<BTreeMap<usize, Arc<rayon::ThreadPool>>>> = OnceLock::new();

/// Number of background pool threads backing a `workers` total-thread budget:
/// `0` when `workers <= 1`, and `workers - 1` when `workers >= 2` (since the
/// calling thread executes the head chunk inline via `in_place_scope`).
#[must_use]
pub const fn pool_thread_count(workers: usize) -> usize {
    workers.saturating_sub(1)
}

/// Scoped task spawner provided to [`scope_workers`] and [`try_scope_workers`].
pub struct WorkerScope<'scope, 'pool> {
    inner: Option<&'pool rayon::Scope<'scope>>,
    /// Invariant marker matching [`rayon::Scope<'scope>`].
    _scope: PhantomData<fn(&'scope ()) -> &'scope ()>,
}

impl<'scope> WorkerScope<'scope, '_> {
    /// Spawn a scoped task onto the cached private worker pool, or execute it
    /// inline immediately when running on the `workers <= 1` serial path.
    pub fn spawn<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'scope,
    {
        match self.inner {
            Some(scope) => scope.spawn(move |_| task()),
            None => task(),
        }
    }

    /// Whether this scope is backed by a multi-thread private pool (`workers >= 2`).
    #[must_use]
    pub const fn is_parallel(&self) -> bool {
        self.inner.is_some()
    }
}

/// Execute `body` inside a private scoped worker budget of `workers` total
/// threads (calling thread plus `workers - 1` cached background pool threads).
///
/// When `workers <= 1`, executes directly on the calling thread without
/// creating or touching any pool; calls to [`WorkerScope::spawn`] run inline.
///
/// # Errors
/// [`CoreError::StateConflict`] if building the cached pool fails or if any
/// task (or `body` itself) panics.
pub fn scope_workers<'scope, F, R>(
    workers: usize,
    operation: &'static str,
    body: F,
) -> Result<R, CoreError>
where
    F: FnOnce(&WorkerScope<'scope, '_>) -> R,
{
    match pool_for(workers, operation)? {
        None => {
            let scope = WorkerScope {
                inner: None,
                _scope: PhantomData,
            };
            catch_unwind(AssertUnwindSafe(|| body(&scope))).map_err(|_| worker_panicked(operation))
        }
        Some(pool) => catch_unwind(AssertUnwindSafe(|| {
            pool.in_place_scope(|rayon_scope| {
                let _guard = DispatchGuard::enter();
                let scope = WorkerScope {
                    inner: Some(rayon_scope),
                    _scope: PhantomData,
                };
                body(&scope)
            })
        }))
        .map_err(|_| worker_panicked(operation)),
    }
}

/// Fallible counterpart to [`scope_workers`], flattening `Result<R, CoreError>`.
///
/// # Errors
/// Returns `body`'s error, or [`CoreError::StateConflict`] if pool creation
/// fails or any task panics.
pub fn try_scope_workers<'scope, F, R>(
    workers: usize,
    operation: &'static str,
    body: F,
) -> Result<R, CoreError>
where
    F: FnOnce(&WorkerScope<'scope, '_>) -> Result<R, CoreError>,
{
    scope_workers(workers, operation, body)?
}

/// Apply `f(index, item)` across every element of `items`, running item `0` on
/// the calling thread and spawning items `1..items.len()` onto the cached
/// private `workers` pool.
///
/// When `workers <= 1` or `items.len() <= 1`, runs serially inline with zero
/// pool lookup.
///
/// # Errors
/// [`CoreError::StateConflict`] if pool creation fails or any worker panics.
pub fn parallel_for_each_mut<T, F>(
    workers: usize,
    operation: &'static str,
    items: &mut [T],
    f: F,
) -> Result<(), CoreError>
where
    T: Send,
    F: Fn(usize, &mut T) + Sync,
{
    if items.is_empty() {
        return Ok(());
    }
    if workers <= 1 || items.len() == 1 || in_worker_dispatch() {
        return catch_unwind(AssertUnwindSafe(|| {
            for (index, item) in items.iter_mut().enumerate() {
                f(index, item);
            }
        }))
        .map_err(|_| worker_panicked(operation));
    }
    let Some((head, tail)) = items.split_first_mut() else {
        return Ok(());
    };
    let f = &f;
    scope_workers(workers, operation, |scope| {
        for (offset, item) in tail.iter_mut().enumerate() {
            let index = offset + 1;
            scope.spawn(move || f(index, item));
        }
        f(0, head);
    })
}

/// Fallible counterpart to [`parallel_for_each_mut`], returning the first
/// error in deterministic ascending index order (identical to serial execution).
///
/// # Errors
/// Returns the lowest-index error produced by `f`, or
/// [`CoreError::StateConflict`] if pool creation fails or any worker panics.
pub fn try_parallel_for_each_mut<T, F>(
    workers: usize,
    operation: &'static str,
    items: &mut [T],
    f: F,
) -> Result<(), CoreError>
where
    T: Send,
    F: Fn(usize, &mut T) -> Result<(), CoreError> + Sync,
{
    if items.is_empty() {
        return Ok(());
    }
    if workers <= 1 || items.len() == 1 || in_worker_dispatch() {
        return catch_unwind(AssertUnwindSafe(|| {
            for (index, item) in items.iter_mut().enumerate() {
                f(index, item)?;
            }
            Ok(())
        }))
        .map_err(|_| worker_panicked(operation))?;
    }
    let Some((head, tail)) = items.split_first_mut() else {
        return Ok(());
    };
    let mut tail_results: Vec<Result<(), CoreError>> = (0..tail.len()).map(|_| Ok(())).collect();
    let f = &f;
    let head_result = scope_workers(workers, operation, |scope| {
        for (offset, (item, slot)) in tail.iter_mut().zip(tail_results.iter_mut()).enumerate() {
            let index = offset + 1;
            scope.spawn(move || {
                *slot = f(index, item);
            });
        }
        f(0, head)
    })?;
    head_result?;
    for result in tail_results {
        result?;
    }
    Ok(())
}

/// Execute `f(index, task)` across every element of `tasks` using dynamic
/// work-stealing across up to `workers` total threads (calling thread plus
/// `workers - 1` cached private pool threads).
///
/// Unlike static one-chunk-per-worker partitioning, dynamic work-stealing lets
/// faster cores (e.g., Apple Silicon P-cores or lightly loaded cells) pull more
/// tiles than slower cores (E-cores or hotspot tiles), eliminating barrier tail
/// stalls while keeping every output index deterministic.
///
/// # Errors
/// [`CoreError::StateConflict`] if pool creation fails or any worker panics.
pub fn parallel_tasks<T, F>(
    workers: usize,
    operation: &'static str,
    tasks: &mut [T],
    f: F,
) -> Result<(), CoreError>
where
    T: Send,
    F: Fn(usize, &mut T) + Sync,
{
    if tasks.is_empty() {
        return Ok(());
    }
    if workers <= 1 || tasks.len() == 1 || in_worker_dispatch() {
        return catch_unwind(AssertUnwindSafe(|| {
            for (index, task) in tasks.iter_mut().enumerate() {
                f(index, task);
            }
        }))
        .map_err(|_| worker_panicked(operation));
    }
    let active = workers.min(tasks.len());
    let queue = Mutex::new(tasks.iter_mut().enumerate());
    let queue_ref = &queue;
    let f = &f;
    let drain = || {
        loop {
            let next = {
                let mut guard = lock_or_recover(queue_ref);
                guard.next()
            };
            let Some((index, task)) = next else {
                break;
            };
            f(index, task);
        }
    };
    scope_workers(workers, operation, |scope| {
        for _ in 1..active {
            scope.spawn(drain);
        }
        drain();
    })
}

/// Fallible counterpart to [`parallel_tasks`], returning the lowest-index error
/// deterministically (identical to serial execution).
///
/// # Errors
/// Returns the lowest-index error produced by `f`, or
/// [`CoreError::StateConflict`] if pool creation fails or any worker panics.
pub fn try_parallel_tasks<T, F>(
    workers: usize,
    operation: &'static str,
    tasks: &mut [T],
    f: F,
) -> Result<(), CoreError>
where
    T: Send,
    F: Fn(usize, &mut T) -> Result<(), CoreError> + Sync,
{
    if tasks.is_empty() {
        return Ok(());
    }
    if workers <= 1 || tasks.len() == 1 || in_worker_dispatch() {
        return catch_unwind(AssertUnwindSafe(|| {
            for (index, task) in tasks.iter_mut().enumerate() {
                f(index, task)?;
            }
            Ok(())
        }))
        .map_err(|_| worker_panicked(operation))?;
    }
    let active = workers.min(tasks.len());
    let mut results: Vec<Result<(), CoreError>> = (0..tasks.len()).map(|_| Ok(())).collect();
    let queue = Mutex::new(tasks.iter_mut().zip(results.iter_mut()).enumerate());
    let queue_ref = &queue;
    let f = &f;
    let drain = || {
        loop {
            let next = {
                let mut guard = lock_or_recover(queue_ref);
                guard.next()
            };
            let Some((index, (task, slot))) = next else {
                break;
            };
            *slot = f(index, task);
        }
    };
    scope_workers(workers, operation, |scope| {
        for _ in 1..active {
            scope.spawn(drain);
        }
        drain();
    })?;
    for result in results {
        result?;
    }
    Ok(())
}

/// Fallible dynamic work-stealing over `tasks`, giving each active thread
/// exclusive access to one reusable per-worker scratch lane from `lanes`.
///
/// Each active thread (`min(workers, lanes.len(), tasks.len())`) keeps its own
/// `&mut L` across all tiles it steals, so scratch buffers stay hot in L1/L2
/// with zero extra allocation while P-cores and E-cores balance dynamically.
/// Returns the lowest-index error deterministically.
///
/// # Errors
/// Returns the lowest-index error produced by `f`, or
/// [`CoreError::StateConflict`] if `lanes` is empty when `tasks` is non-empty,
/// pool creation fails, or any worker panics.
pub fn try_parallel_tasks_with_lanes<L, T, F>(
    workers: usize,
    operation: &'static str,
    lanes: &mut [L],
    tasks: &mut [T],
    f: F,
) -> Result<(), CoreError>
where
    L: Send,
    T: Send,
    F: Fn(usize, &mut T, &mut L) -> Result<(), CoreError> + Sync,
{
    if tasks.is_empty() {
        return Ok(());
    }
    let Some((first_lane, rest_lanes)) = lanes.split_first_mut() else {
        return Err(CoreError::StateConflict {
            operation,
            detail: "no worker lane".to_owned(),
        });
    };
    let active = workers.min(lanes.len()).min(tasks.len()).max(1);
    if active == 1 || in_worker_dispatch() {
        return catch_unwind(AssertUnwindSafe(|| {
            for (index, task) in tasks.iter_mut().enumerate() {
                f(index, task, first_lane)?;
            }
            Ok(())
        }))
        .map_err(|_| worker_panicked(operation))?;
    }
    let mut results: Vec<Result<(), CoreError>> = (0..tasks.len()).map(|_| Ok(())).collect();
    let queue = Mutex::new(tasks.iter_mut().zip(results.iter_mut()).enumerate());
    let queue_ref = &queue;
    let f = &f;
    let drain_with = |lane: &mut L| {
        loop {
            let next = {
                let mut guard = lock_or_recover(queue_ref);
                guard.next()
            };
            let Some((index, (task, slot))) = next else {
                break;
            };
            *slot = f(index, task, lane);
        }
    };
    let drain_with = &drain_with;
    scope_workers(workers, operation, |scope| {
        for lane in rest_lanes.iter_mut().take(active - 1) {
            scope.spawn(move || drain_with(lane));
        }
        drain_with(first_lane);
    })?;
    for result in results {
        result?;
    }
    Ok(())
}

/// Target work-stealing tiles per worker for chunked parallel loops.
const TILES_PER_WORKER: usize = 4;
/// Upper cap on work-stealing tiles so queue pops stay sub-microsecond.
const MAX_WORK_TILES: usize = 64;

/// Split `slice` into contiguous tiles (each at least `min_chunk.max(1)`
/// elements, except possibly the final tile) and invoke `f(start_index, tile)`
/// using dynamic work-stealing across up to `workers` total threads.
///
/// # Errors
/// [`CoreError::StateConflict`] if pool creation fails or any worker panics.
pub fn parallel_chunks_mut<T, F>(
    workers: usize,
    operation: &'static str,
    slice: &mut [T],
    min_chunk: usize,
    f: F,
) -> Result<(), CoreError>
where
    T: Send,
    F: Fn(usize, &mut [T]) + Sync,
{
    let len = slice.len();
    if len == 0 {
        return Ok(());
    }
    let max_chunks = len.div_ceil(min_chunk.max(1)).max(1);
    let used = workers.max(1).min(max_chunks);
    if used == 1 || in_worker_dispatch() {
        return catch_unwind(AssertUnwindSafe(|| f(0, slice)))
            .map_err(|_| worker_panicked(operation));
    }
    let target_tiles = used
        .saturating_mul(TILES_PER_WORKER)
        .min(MAX_WORK_TILES)
        .min(max_chunks)
        .max(used);
    let tile_len = len.div_ceil(target_tiles);
    let mut tiles: Vec<(usize, &mut [T])> = slice
        .chunks_mut(tile_len)
        .enumerate()
        .map(|(idx, chunk)| (idx * tile_len, chunk))
        .collect();
    parallel_tasks(workers, operation, &mut tiles, |_, (start, chunk)| {
        f(*start, chunk);
    })
}

/// Fallible counterpart to [`parallel_chunks_mut`], returning the first error
/// in deterministic ascending chunk order.
///
/// # Errors
/// Returns the lowest-offset error produced by `f`, or
/// [`CoreError::StateConflict`] if pool creation fails or any worker panics.
pub fn try_parallel_chunks_mut<T, F>(
    workers: usize,
    operation: &'static str,
    slice: &mut [T],
    min_chunk: usize,
    f: F,
) -> Result<(), CoreError>
where
    T: Send,
    F: Fn(usize, &mut [T]) -> Result<(), CoreError> + Sync,
{
    let len = slice.len();
    if len == 0 {
        return Ok(());
    }
    let max_chunks = len.div_ceil(min_chunk.max(1)).max(1);
    let used = workers.max(1).min(max_chunks);
    if used == 1 || in_worker_dispatch() {
        return catch_unwind(AssertUnwindSafe(|| f(0, slice)))
            .map_err(|_| worker_panicked(operation))?;
    }
    let target_tiles = used
        .saturating_mul(TILES_PER_WORKER)
        .min(MAX_WORK_TILES)
        .min(max_chunks)
        .max(used);
    let tile_len = len.div_ceil(target_tiles);
    let mut tiles: Vec<(usize, &mut [T])> = slice
        .chunks_mut(tile_len)
        .enumerate()
        .map(|(idx, chunk)| (idx * tile_len, chunk))
        .collect();
    try_parallel_tasks(workers, operation, &mut tiles, |_, (start, chunk)| {
        f(*start, chunk)
    })
}

/// Handle to a cached private Rayon pool (zero-refcount static borrow for
/// `2..=MAX_FAST_WORKERS`, `Arc` clone for larger overflow counts).
enum PoolRef {
    Static(&'static rayon::ThreadPool),
    Owned(Arc<rayon::ThreadPool>),
}

impl Deref for PoolRef {
    type Target = rayon::ThreadPool;

    #[inline]
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Static(pool) => pool,
            Self::Owned(pool) => pool,
        }
    }
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn worker_panicked(operation: &'static str) -> CoreError {
    CoreError::StateConflict {
        operation,
        detail: "a worker panicked".to_owned(),
    }
}

fn build_pool(
    workers: usize,
    operation: &'static str,
) -> Result<Arc<rayon::ThreadPool>, CoreError> {
    let pool_threads = pool_thread_count(workers);
    rayon::ThreadPoolBuilder::new()
        .num_threads(pool_threads)
        .thread_name(move |index| format!("flife-w{workers}-{index}"))
        .start_handler(|_| ON_POOL_THREAD.set(true))
        .build()
        .map(Arc::new)
        .map_err(|error| CoreError::StateConflict {
            operation,
            detail: format!("failed to build {pool_threads}-thread worker pool: {error}"),
        })
}

fn pool_for(workers: usize, operation: &'static str) -> Result<Option<PoolRef>, CoreError> {
    if workers <= 1 || in_worker_dispatch() {
        return Ok(None);
    }
    if let Some(slot) = FAST_POOLS.get(workers - 2) {
        if let Some(pool) = slot.get() {
            return Ok(Some(PoolRef::Static(pool)));
        }
        let _guard = lock_or_recover(FAST_INIT_LOCK.get_or_init(|| Mutex::new(())));
        if let Some(pool) = slot.get() {
            return Ok(Some(PoolRef::Static(pool)));
        }
        let built = build_pool(workers, operation)?;
        let _ = slot.set(built);
        let pool = slot.get().ok_or_else(|| CoreError::StateConflict {
            operation,
            detail: "cached worker pool slot uninitialized after set".to_owned(),
        })?;
        return Ok(Some(PoolRef::Static(pool)));
    }

    let map = OVERFLOW_POOLS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut pools = lock_or_recover(map);
    if let Some(existing) = pools.get(&workers) {
        return Ok(Some(PoolRef::Owned(Arc::clone(existing))));
    }
    let built = build_pool(workers, operation)?;
    pools.insert(workers, Arc::clone(&built));
    Ok(Some(PoolRef::Owned(built)))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Barrier;
    use std::thread;

    use super::*;

    #[test]
    fn serial_workers_one_runs_inline_without_pool_threads() {
        assert_eq!(pool_thread_count(0), 0);
        assert_eq!(pool_thread_count(1), 0);
        assert_eq!(pool_thread_count(8), 7);

        let caller = thread::current().id();
        let mut spawned_tid = None;
        let tid_slot = &mut spawned_tid;
        let was_parallel = scope_workers(1, "serial test", |scope| {
            scope.spawn(move || {
                *tid_slot = Some(thread::current().id());
            });
            scope.is_parallel()
        })
        .expect("workers=1 succeeds");
        assert!(!was_parallel);
        assert_eq!(spawned_tid, Some(caller));
    }

    #[test]
    fn worker_budget_bound_and_thread_reuse_across_waves() {
        // Dedicated worker count (5) so parallel test threads never contend with the Barrier.
        let workers = 5_usize;
        let caller = thread::current().id();
        let mut all_threads = HashSet::new();
        let mut first_wave_threads = HashSet::new();

        for wave in 0..16_usize {
            let barrier = Barrier::new(workers);
            let mut tids = [None; 5];
            let mut names: [Option<String>; 5] = [None, None, None, None, None];
            let barrier_ref = &barrier;
            let (head_tid, tail_tids) = tids.split_first_mut().expect("non-empty");
            let (head_name, tail_names) = names.split_first_mut().expect("non-empty");
            scope_workers(workers, "budget test", |scope| {
                assert!(scope.is_parallel());
                for (tid_slot, name_slot) in tail_tids.iter_mut().zip(tail_names.iter_mut()) {
                    scope.spawn(move || {
                        barrier_ref.wait();
                        *tid_slot = Some(thread::current().id());
                        *name_slot = thread::current().name().map(str::to_owned);
                    });
                }
                barrier_ref.wait();
                *head_tid = Some(thread::current().id());
                *head_name = thread::current().name().map(str::to_owned);
            })
            .expect("5-worker wave succeeds");

            // Head ran on caller; every background worker belongs to our private `flife-w5-*` pool.
            assert_eq!(tids[0], Some(caller));
            for name in &names[1..] {
                let name = name.as_deref().expect("named pool thread");
                assert!(
                    name.starts_with("flife-w5-"),
                    "expected private flife-w5-* worker, got `{name}`"
                );
            }

            let wave_set: HashSet<_> = tids.into_iter().flatten().collect();
            assert_eq!(
                wave_set.len(),
                workers,
                "all 5 threads (1 caller + 4 pool) must participate concurrently"
            );
            if wave == 0 {
                first_wave_threads = wave_set.clone();
            } else {
                assert_eq!(
                    wave_set, first_wave_threads,
                    "cached pool must reuse exact same OS threads across waves"
                );
            }
            all_threads.extend(wave_set);
        }

        // Across 16 waves, total distinct OS threads never exceeded the 5-thread budget.
        assert_eq!(all_threads.len(), workers);
    }

    #[test]
    fn panics_in_worker_or_caller_return_state_conflict_and_preserve_pool_health() {
        // 1. Panic inside a spawned worker on workers=4.
        let worker_err = scope_workers(4, "worker panic op", |scope| {
            scope.spawn(|| {
                panic!("synthetic worker panic");
            });
        })
        .expect_err("worker panic must return Err");
        assert!(matches!(
            worker_err,
            CoreError::StateConflict {
                operation: "worker panic op",
                ..
            }
        ));

        // 2. Panic inside the caller body on both workers=4 and workers=1.
        for workers in [1_usize, 4] {
            let caller_err = scope_workers(workers, "caller panic op", |_scope| {
                panic!("synthetic caller panic");
            })
            .expect_err("caller panic must return Err");
            assert!(matches!(
                caller_err,
                CoreError::StateConflict {
                    operation: "caller panic op",
                    ..
                }
            ));
        }

        // 3. The cached workers=4 pool stays completely healthy for subsequent calls.
        let mut data = [0_u64; 64];
        parallel_chunks_mut(4, "post-panic check", &mut data, 4, |start, chunk| {
            for (offset, slot) in chunk.iter_mut().enumerate() {
                *slot = (start + offset) as u64 * 3 + 1;
            }
        })
        .expect("pool remains healthy after panic");
        for (i, &value) in data.iter().enumerate() {
            assert_eq!(value, i as u64 * 3 + 1);
        }
    }

    #[test]
    fn try_helpers_preserve_deterministic_first_error_order() {
        let mut items = [0_usize, 1, 2, 3, 4, 5];
        let err = try_parallel_for_each_mut(4, "for_each_err", &mut items, |idx, slot| {
            *slot = idx * 10;
            if idx == 2 || idx == 5 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "item",
                    detail: format!("failed at {idx}"),
                });
            }
            Ok(())
        })
        .expect_err("must report error");
        assert!(
            err.to_string().contains("failed at 2"),
            "must return lowest-index error deterministically, got: {err}"
        );

        let mut buf = [0_u32; 32];
        let err = try_parallel_chunks_mut(4, "chunks_err", &mut buf, 4, |start, chunk| {
            for (i, slot) in chunk.iter_mut().enumerate() {
                *slot = (start + i) as u32;
            }
            if start == 8 || start == 24 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "chunk",
                    detail: format!("chunk at {start}"),
                });
            }
            Ok(())
        })
        .expect_err("must report chunk error");
        assert!(
            err.to_string().contains("chunk at 8"),
            "must return lowest-offset chunk error deterministically, got: {err}"
        );
    }

    #[test]
    fn parallel_chunks_and_for_each_cover_edge_sizes() {
        let mut empty: [u32; 0] = [];
        parallel_chunks_mut(4, "empty", &mut empty, 1, |_, _| {
            panic!("must not run on empty slice");
        })
        .expect("empty chunks");
        parallel_for_each_mut(4, "empty", &mut empty, |_, _| {
            panic!("must not run on empty slice");
        })
        .expect("empty for_each");

        for len in [1_usize, 2, 3, 5, 10, 31, 64] {
            for min_chunk in [0_usize, 1, 3, 16, 100] {
                let mut got = vec![0_usize; len];
                parallel_chunks_mut(6, "edge chunks", &mut got, min_chunk, |start, chunk| {
                    for (offset, slot) in chunk.iter_mut().enumerate() {
                        *slot = start + offset + 100;
                    }
                })
                .expect("edge chunks ok");
                let expected: Vec<usize> = (0..len).map(|i| i + 100).collect();
                assert_eq!(got, expected, "len={len}, min_chunk={min_chunk}");
            }
        }
    }

    #[test]
    fn private_pool_does_not_interfere_with_independent_rayon_pools() {
        let other = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .thread_name(|i| format!("external-rayon-{i}"))
            .build()
            .expect("builds independent rayon pool");

        let external_name = other.install(|| {
            let mut seen_private = None;
            let seen_slot = &mut seen_private;
            scope_workers(3, "coexist", |scope| {
                scope.spawn(move || {
                    *seen_slot = thread::current().name().map(str::to_owned);
                });
            })
            .expect("private pool runs cleanly inside external pool");
            assert!(
                seen_private
                    .as_deref()
                    .is_some_and(|name| name.starts_with("flife-w3-")),
                "expected flife-w3-* private worker, got {seen_private:?}"
            );
            thread::current().name().map(str::to_owned)
        });
        assert!(
            external_name
                .as_deref()
                .is_some_and(|name| name.starts_with("external-rayon-")),
            "external pool thread unaffected, got {external_name:?}"
        );
    }

    #[test]
    fn nested_dispatch_from_worker_or_caller_falls_back_inline_without_deadlock() {
        let mut worker_nested_parallel = None;
        let worker_slot = &mut worker_nested_parallel;
        let caller_nested_parallel = scope_workers(4, "outer", |outer| {
            assert!(outer.is_parallel());
            outer.spawn(move || {
                let inner_par = scope_workers(4, "inner_from_worker", |inner| {
                    let mut buf = [0_u32; 8];
                    parallel_chunks_mut(4, "inner_chunks", &mut buf, 2, |start, chunk| {
                        for (i, slot) in chunk.iter_mut().enumerate() {
                            *slot = (start + i) as u32 + 1;
                        }
                    })
                    .expect("nested chunks succeed inline");
                    assert_eq!(buf, [1, 2, 3, 4, 5, 6, 7, 8]);
                    inner.is_parallel()
                })
                .expect("nested worker scope succeeds inline");
                *worker_slot = Some(inner_par);
            });
            scope_workers(4, "inner_from_caller", |inner| inner.is_parallel())
                .expect("nested caller scope succeeds inline")
        })
        .expect("outer scope succeeds");

        assert_eq!(worker_nested_parallel, Some(false));
        assert!(!caller_nested_parallel);

        // After unwinding/exiting the outer scope, top-level dispatch is parallel again.
        let top_level_parallel =
            scope_workers(4, "after_nested", |scope| scope.is_parallel()).expect("top-level ok");
        assert!(top_level_parallel);
    }
}
