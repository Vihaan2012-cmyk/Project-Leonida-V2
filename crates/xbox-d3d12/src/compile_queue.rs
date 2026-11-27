//! Pipeline compilation off the render thread, with the persistent cache warmed at start.
//!
//! The render thread asks for a pipeline by key with [`CompileQueue::request`] and gets one of three answers
//! at once: ready, still building (skip the draw this frame), or failed. A pool of workers builds through a
//! callback; a key is built once however many callers ask, and what is needed now goes ahead of prefetching,
//! which goes ahead of the start-up warm list.
//!
//! The number of builds at once is the number of workers, and a memory budget caps what they may weigh together:
//! a build whose weight does not fit waits for the running ones to finish, and one heavier than the whole budget
//! runs alone. A draw whose pipeline is not ready either [`waits`](CompileQueue::wait) for that one build or
//! skips ([`DrawPolicy`]).

mod seen;
mod service;

pub use seen::{BytecodeStore, MemoryBytecode, Pso, PsoKey, SeenEntry, SeenError, SeenLog};
pub use service::{
    PipelineBuilder, PipelineHandle, PipelineService, Resolved, ServiceConfig, ServiceStats,
};

use std::collections::{BinaryHeap, HashMap, HashSet};
use std::hash::Hash;
use std::io::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use thiserror::Error;

/// How soon a pipeline is wanted. Higher is built first; equal priorities build in request order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// The start-up list of every pipeline the shader store knows.
    Warm,
    /// Expected soon (the next area, the next material).
    Prefetch,
    /// A draw is waiting for it.
    Needed,
}

/// What a draw does when its pipeline is not ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawPolicy {
    /// Wait for that pipeline's own build, whatever else is queued. Nothing is missing from the picture; the
    /// frame takes as long as the build.
    #[default]
    Wait,
    /// Leave the draw out and ask again next frame. No stall, at the cost of a missing object for a frame or two.
    Skip,
}

impl std::str::FromStr for DrawPolicy {
    type Err = String;

    fn from_str(text: &str) -> Result<DrawPolicy, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "wait" => Ok(DrawPolicy::Wait),
            "skip" => Ok(DrawPolicy::Skip),
            other => Err(format!("`{other}` is not a draw policy (wait or skip)")),
        }
    }
}

/// Why a build failed, as the builder reports it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct BuildError(pub String);

impl From<String> for BuildError {
    fn from(text: String) -> Self {
        BuildError(text)
    }
}

impl From<&str> for BuildError {
    fn from(text: &str) -> Self {
        BuildError(text.to_string())
    }
}

/// What the queue knows about one key.
#[derive(Debug)]
pub enum Poll<P> {
    Ready(Arc<P>),
    /// Queued or building: skip the draw and ask again next frame.
    Pending,
    /// The builder failed (or panicked); not retried until [`CompileQueue::forget`].
    Failed(Arc<str>),
}

impl<P> Clone for Poll<P> {
    fn clone(&self) -> Self {
        match self {
            Poll::Ready(p) => Poll::Ready(p.clone()),
            Poll::Pending => Poll::Pending,
            Poll::Failed(e) => Poll::Failed(e.clone()),
        }
    }
}

impl<P> Poll<P> {
    pub fn ready(&self) -> Option<&Arc<P>> {
        match self {
            Poll::Ready(p) => Some(p),
            _ => None,
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, Poll::Pending)
    }
}

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("pipeline cache file {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("nothing to save: the snapshot callback returned no data")]
    NoSnapshot,
}

/// The persistent pipeline cache file: the driver's cache blob with a header that makes a stale or damaged
/// file read as absent. `stamp` identifies what the blob is valid for (driver, device, our build); a file
/// written under another stamp is ignored.
#[derive(Debug, Clone)]
pub struct CacheFile {
    path: PathBuf,
    stamp: u64,
}

const CACHE_MAGIC: [u8; 4] = *b"LPSC";
const CACHE_VERSION: u32 = 1;
const CACHE_HEADER: usize = 4 + 4 + 8 + 8 + 8;

fn checksum(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

impl CacheFile {
    pub fn new(path: impl Into<PathBuf>, stamp: u64) -> CacheFile {
        CacheFile {
            path: path.into(),
            stamp,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The saved blob, or `None` when the file is missing, damaged or from another stamp.
    pub fn load(&self) -> Option<Vec<u8>> {
        let bytes = std::fs::read(&self.path).ok()?;
        if bytes.len() < CACHE_HEADER || bytes[..4] != CACHE_MAGIC {
            return None;
        }
        let word = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes"));
        let version = u32::from_le_bytes(bytes[4..8].try_into().expect("4 bytes"));
        let (stamp, length, sum) = (word(8), word(16), word(24));
        let body = &bytes[CACHE_HEADER..];
        (version == CACHE_VERSION
            && stamp == self.stamp
            && length == body.len() as u64
            && sum == checksum(body))
        .then(|| body.to_vec())
    }

    /// Writes the blob through a temporary file and a rename, so a crash leaves the old file.
    pub fn store(&self, blob: &[u8]) -> Result<(), CacheError> {
        let io = |source| CacheError::Io {
            path: self.path.clone(),
            source,
        };
        if let Some(dir) = self.path.parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(".tmp");
        let temporary = self.path.with_file_name(name);
        let mut bytes = Vec::with_capacity(CACHE_HEADER + blob.len());
        bytes.extend_from_slice(&CACHE_MAGIC);
        bytes.extend_from_slice(&CACHE_VERSION.to_le_bytes());
        bytes.extend_from_slice(&self.stamp.to_le_bytes());
        bytes.extend_from_slice(&(blob.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&checksum(blob).to_le_bytes());
        bytes.extend_from_slice(blob);
        let mut file = std::fs::File::create(&temporary).map_err(io)?;
        file.write_all(&bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        std::fs::rename(&temporary, &self.path).map_err(io)
    }
}

/// Where and when the driver's cache blob is saved. `snapshot` returns the blob (`vkGetPipelineCacheData`);
/// the queue saves when it goes idle after building something, and after every `every` builds in a long run.
pub struct Persist {
    pub file: CacheFile,
    pub snapshot: Box<dyn Fn() -> Option<Vec<u8>> + Send + Sync>,
    pub every: u64,
}

pub struct Config {
    /// Worker threads; at least one.
    pub workers: usize,
    /// Runs first on each worker thread, for example to lower its priority.
    pub worker_init: Option<Arc<dyn Fn() + Send + Sync>>,
    pub persist: Option<Persist>,
    /// The most the builds running together may weigh, by the queue's weight function; `None` for no cap.
    pub memory_budget: Option<u64>,
}

impl Default for Config {
    /// Leaves two processors to the game's own threads, and at least one worker.
    fn default() -> Config {
        let cpus = std::thread::available_parallelism().map_or(2, |n| n.get());
        Config {
            workers: cpus.saturating_sub(2).clamp(1, 8),
            worker_init: None,
            persist: None,
            memory_budget: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub queued: usize,
    pub building: usize,
    pub ready: usize,
    pub failed: usize,
    /// Builds finished successfully since the start.
    pub built: u64,
    /// Keys given to [`CompileQueue::warm`] and how many of them have finished (ready or failed).
    pub warm_total: usize,
    pub warm_done: usize,
    pub flushes: u64,
    pub flush_errors: u64,
    /// The weight of the builds running now, and the most builds and weight there have been at once.
    pub memory_in_use: u64,
    pub peak_building: usize,
    pub peak_memory: u64,
}

enum Slot<P> {
    Queued(Priority),
    Building,
    Ready(Arc<P>),
    Failed(Arc<str>),
}

struct Job<K> {
    priority: Priority,
    seq: u64,
    key: K,
}

impl<K> PartialEq for Job<K> {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.seq == other.seq
    }
}
impl<K> Eq for Job<K> {}
impl<K> PartialOrd for Job<K> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<K> Ord for Job<K> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority
            .cmp(&other.priority)
            .then(other.seq.cmp(&self.seq))
    }
}

struct Inner<K, P> {
    slots: HashMap<K, Slot<P>>,
    heap: BinaryHeap<Job<K>>,
    warm: HashSet<K>,
    seq: u64,
    shutdown: bool,
    stats: Stats,
    since_flush: u64,
}

type Builder<K, P> = dyn Fn(&K) -> Result<P, BuildError> + Send + Sync;
type Weigher<K> = dyn Fn(&K) -> u64 + Send + Sync;

struct Shared<K, P> {
    inner: Mutex<Inner<K, P>>,
    work: Condvar,
    done: Condvar,
    builder: Box<Builder<K, P>>,
    weigh: Option<Box<Weigher<K>>>,
    budget: Option<u64>,
    persist: Option<Persist>,
}

impl<K: Hash + Eq + Clone, P> Shared<K, P> {
    fn lock(&self) -> MutexGuard<'_, Inner<K, P>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn enqueue(&self, inner: &mut Inner<K, P>, key: K, priority: Priority) -> bool {
        match inner.slots.get_mut(&key) {
            None => {
                inner.slots.insert(key.clone(), Slot::Queued(priority));
                inner.stats.queued += 1;
            }
            Some(Slot::Queued(current)) if priority > *current => *current = priority,
            Some(_) => return false,
        }
        inner.seq += 1;
        let seq = inner.seq;
        inner.heap.push(Job { priority, seq, key });
        true
    }

    fn flush(&self) -> Result<(), CacheError> {
        let Some(persist) = &self.persist else {
            return Ok(());
        };
        let result = match (persist.snapshot)() {
            Some(blob) => persist.file.store(&blob),
            None => Err(CacheError::NoSnapshot),
        };
        let mut inner = self.lock();
        match result {
            Ok(()) => inner.stats.flushes += 1,
            Err(_) => inner.stats.flush_errors += 1,
        }
        result
    }

    fn run(&self) {
        loop {
            let mut inner = self.lock();
            let (key, weight) = loop {
                if inner.shutdown {
                    return;
                }
                let Some(job) = inner.heap.peek() else {
                    inner = self.work.wait(inner).unwrap_or_else(|e| e.into_inner());
                    continue;
                };
                if !matches!(inner.slots.get(&job.key), Some(Slot::Queued(p)) if *p == job.priority)
                {
                    inner.heap.pop();
                    continue;
                }
                let weight = self.weigh.as_ref().map_or(0, |weigh| weigh(&job.key));
                let fits = self.budget.is_none_or(|budget| {
                    inner.stats.building == 0 || inner.stats.memory_in_use + weight <= budget
                });
                if !fits {
                    inner = self.work.wait(inner).unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                let job = inner.heap.pop().expect("peeked");
                inner.slots.insert(job.key.clone(), Slot::Building);
                inner.stats.queued -= 1;
                inner.stats.building += 1;
                inner.stats.memory_in_use += weight;
                inner.stats.peak_building = inner.stats.peak_building.max(inner.stats.building);
                inner.stats.peak_memory = inner.stats.peak_memory.max(inner.stats.memory_in_use);
                break (job.key, weight);
            };
            drop(inner);

            let result = catch_unwind(AssertUnwindSafe(|| (self.builder)(&key)));

            let mut inner = self.lock();
            inner.stats.building -= 1;
            inner.stats.memory_in_use -= weight;
            let slot = match result {
                Ok(Ok(pipeline)) => {
                    inner.stats.built += 1;
                    inner.stats.ready += 1;
                    Slot::Ready(Arc::new(pipeline))
                }
                Ok(Err(error)) => {
                    inner.stats.failed += 1;
                    Slot::Failed(error.0.into())
                }
                Err(_) => {
                    inner.stats.failed += 1;
                    Slot::Failed("the pipeline builder panicked".into())
                }
            };
            inner.slots.insert(key.clone(), slot);
            if inner.warm.remove(&key) {
                inner.stats.warm_done += 1;
            }
            inner.since_flush += 1;
            let idle = inner.stats.queued == 0 && inner.stats.building == 0;
            let flush = self
                .persist
                .as_ref()
                .is_some_and(|p| inner.since_flush >= p.every.max(1) || idle);
            if flush {
                inner.since_flush = 0;
            }
            drop(inner);
            self.done.notify_all();
            self.work.notify_all();
            if flush {
                let _ = self.flush();
                self.done.notify_all();
            }
        }
    }
}

/// A pool of workers building pipelines for keys of type `K` into values of type `P`.
pub struct CompileQueue<K, P> {
    shared: Arc<Shared<K, P>>,
    workers: Vec<JoinHandle<()>>,
}

impl<K, P> CompileQueue<K, P>
where
    K: Hash + Eq + Clone + Send + 'static,
    P: Send + Sync + 'static,
{
    pub fn new(
        config: Config,
        builder: impl Fn(&K) -> Result<P, BuildError> + Send + Sync + 'static,
    ) -> CompileQueue<K, P> {
        Self::start(config, None, builder)
    }

    /// As [`new`](Self::new), with a weight for each key, which the memory budget caps across running builds.
    /// `weigh` runs under the queue's lock: it must be quick and must not call the queue.
    pub fn weighted(
        config: Config,
        weigh: impl Fn(&K) -> u64 + Send + Sync + 'static,
        builder: impl Fn(&K) -> Result<P, BuildError> + Send + Sync + 'static,
    ) -> CompileQueue<K, P> {
        Self::start(config, Some(Box::new(weigh)), builder)
    }

    fn start(
        config: Config,
        weigh: Option<Box<Weigher<K>>>,
        builder: impl Fn(&K) -> Result<P, BuildError> + Send + Sync + 'static,
    ) -> CompileQueue<K, P> {
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                slots: HashMap::new(),
                heap: BinaryHeap::new(),
                warm: HashSet::new(),
                seq: 0,
                shutdown: false,
                stats: Stats::default(),
                since_flush: 0,
            }),
            work: Condvar::new(),
            done: Condvar::new(),
            builder: Box::new(builder),
            weigh,
            budget: config.memory_budget,
            persist: config.persist,
        });
        let workers = (0..config.workers.max(1))
            .map(|n| {
                let shared = shared.clone();
                let init = config.worker_init.clone();
                std::thread::Builder::new()
                    .name(format!("leonida-pipeline-{n}"))
                    .spawn(move || {
                        if let Some(init) = init {
                            init();
                        }
                        shared.run();
                    })
                    .expect("spawning a worker thread")
            })
            .collect();
        CompileQueue { shared, workers }
    }

    /// The pipeline for `key`, queuing a build at `priority` if nobody has asked yet, or moving a queued
    /// lower-priority build up. Never blocks on a build.
    pub fn request(&self, key: &K, priority: Priority) -> Poll<P> {
        let mut inner = self.shared.lock();
        match inner.slots.get(key) {
            Some(Slot::Ready(p)) => return Poll::Ready(p.clone()),
            Some(Slot::Failed(e)) => return Poll::Failed(e.clone()),
            _ => {}
        }
        if self.shared.enqueue(&mut inner, key.clone(), priority) {
            self.shared.work.notify_one();
        }
        Poll::Pending
    }

    /// What is known about `key`, without queuing anything. `None` for a key nobody has asked for.
    pub fn poll(&self, key: &K) -> Option<Poll<P>> {
        match self.shared.lock().slots.get(key)? {
            Slot::Ready(p) => Some(Poll::Ready(p.clone())),
            Slot::Failed(e) => Some(Poll::Failed(e.clone())),
            Slot::Queued(_) | Slot::Building => Some(Poll::Pending),
        }
    }

    /// Like [`request`](Self::request) at [`Priority::Needed`], then waits up to `timeout` for the result.
    /// For loading screens and for a draw that cannot be skipped.
    pub fn request_wait(&self, key: &K, timeout: Duration) -> Poll<P> {
        self.wait_until(key, Some(Instant::now() + timeout))
    }

    /// Like [`request`](Self::request) at [`Priority::Needed`], then waits for that one build: ready or failed,
    /// never pending. The wait is on this key alone; other builds queue and finish around it.
    pub fn wait(&self, key: &K) -> Poll<P> {
        self.wait_until(key, None)
    }

    /// What a draw that needs `key` gets under `policy`.
    pub fn for_draw(&self, key: &K, policy: DrawPolicy) -> Poll<P> {
        match policy {
            DrawPolicy::Wait => self.wait(key),
            DrawPolicy::Skip => self.request(key, Priority::Needed),
        }
    }

    fn wait_until(&self, key: &K, deadline: Option<Instant>) -> Poll<P> {
        let first = self.request(key, Priority::Needed);
        if !first.is_pending() {
            return first;
        }
        let mut inner = self.shared.lock();
        loop {
            match inner.slots.get(key) {
                Some(Slot::Ready(p)) => return Poll::Ready(p.clone()),
                Some(Slot::Failed(e)) => return Poll::Failed(e.clone()),
                Some(_) => {}
                None => return Poll::Pending,
            }
            inner = match deadline {
                None => self
                    .shared
                    .done
                    .wait(inner)
                    .unwrap_or_else(|e| e.into_inner()),
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Poll::Pending;
                    }
                    self.shared
                        .done
                        .wait_timeout(inner, left)
                        .unwrap_or_else(|e| e.into_inner())
                        .0
                }
            };
        }
    }

    /// Queues `keys` at [`Priority::Prefetch`]; returns how many were new or moved up.
    pub fn prefetch(&self, keys: impl IntoIterator<Item = K>) -> usize {
        self.enqueue_all(keys, Priority::Prefetch, false)
    }

    /// Queues the start-up list, in the order given, at [`Priority::Warm`]. Keys already known are skipped.
    /// Builds go through the same builder, so the driver's cache file fills as they finish.
    pub fn warm(&self, keys: impl IntoIterator<Item = K>) -> usize {
        self.enqueue_all(keys, Priority::Warm, true)
    }

    fn enqueue_all(
        &self,
        keys: impl IntoIterator<Item = K>,
        priority: Priority,
        warm: bool,
    ) -> usize {
        let mut inner = self.shared.lock();
        let mut added = 0;
        for key in keys {
            let known = inner.slots.contains_key(&key);
            if self.shared.enqueue(&mut inner, key.clone(), priority) {
                added += 1;
                if warm && !known {
                    inner.warm.insert(key);
                    inner.stats.warm_total += 1;
                }
            }
        }
        drop(inner);
        for _ in 0..added.min(self.workers.len()) {
            self.shared.work.notify_one();
        }
        added
    }

    /// Drops a finished result (ready or failed) so the next request builds it again. A key still queued or
    /// building is left alone.
    pub fn forget(&self, key: &K) -> bool {
        let mut inner = self.shared.lock();
        match inner.slots.get(key) {
            Some(Slot::Ready(_)) => inner.stats.ready -= 1,
            Some(Slot::Failed(_)) => inner.stats.failed -= 1,
            _ => return false,
        }
        inner.slots.remove(key);
        true
    }

    pub fn stats(&self) -> Stats {
        self.shared.lock().stats
    }

    /// Waits until nothing is queued or building, up to `timeout`; true if it went idle.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut inner = self.shared.lock();
        loop {
            if inner.stats.queued == 0 && inner.stats.building == 0 {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            inner = self
                .shared
                .done
                .wait_timeout(inner, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Saves the driver cache now, if a [`Persist`] was configured.
    pub fn flush(&self) -> Result<(), CacheError> {
        self.shared.flush()
    }
}

impl<K, P> Drop for CompileQueue<K, P> {
    fn drop(&mut self) {
        {
            let mut inner = self.shared.inner.lock().unwrap_or_else(|e| e.into_inner());
            inner.shutdown = true;
        }
        self.shared.work.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    const LONG: Duration = Duration::from_secs(10);

    fn config(workers: usize) -> Config {
        Config {
            workers,
            worker_init: None,
            persist: None,
            memory_budget: None,
        }
    }

    /// A builder that records the order of keys and blocks key 0 until the gate opens.
    struct Fake {
        order: Arc<Mutex<Vec<u32>>>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                order: Arc::default(),
                gate: Arc::new((Mutex::new(false), Condvar::new())),
            }
        }

        fn open(&self) {
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
        }

        fn queue(&self) -> CompileQueue<u32, String> {
            let order = self.order.clone();
            let gate = self.gate.clone();
            CompileQueue::new(config(1), move |key: &u32| {
                order.lock().unwrap().push(*key);
                if *key == 0 {
                    let mut open = gate.0.lock().unwrap();
                    while !*open {
                        open = gate.1.wait(open).unwrap();
                    }
                }
                if *key != 0 {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Ok(format!("pipeline {key}"))
            })
        }

        fn wait_started(&self, key: u32) {
            let start = Instant::now();
            while !self.order.lock().unwrap().contains(&key) {
                assert!(start.elapsed() < LONG, "builder never started");
                std::thread::yield_now();
            }
        }
    }

    #[test]
    fn not_ready_then_ready() {
        let fake = Fake::new();
        let queue = fake.queue();
        assert!(queue.poll(&0).is_none());
        assert!(queue.request(&0, Priority::Needed).is_pending());
        fake.wait_started(0);
        assert!(queue.request(&0, Priority::Needed).is_pending());
        assert!(queue.poll(&0).unwrap().is_pending());
        fake.open();
        assert!(queue.wait_idle(LONG));
        let ready = queue.request(&0, Priority::Needed);
        assert_eq!(ready.ready().unwrap().as_str(), "pipeline 0");
        assert_eq!(queue.stats().ready, 1);
    }

    #[test]
    fn priorities_order_the_builds() {
        let fake = Fake::new();
        let queue = fake.queue();
        queue.request(&0, Priority::Needed);
        fake.wait_started(0);
        queue.warm([10, 11]);
        queue.prefetch([20, 21]);
        queue.request(&30, Priority::Needed);
        queue.request(&31, Priority::Needed);
        fake.open();
        assert!(queue.wait_idle(LONG));
        assert_eq!(*fake.order.lock().unwrap(), vec![0, 30, 31, 20, 21, 10, 11]);
    }

    #[test]
    fn a_later_request_moves_a_queued_key_up_and_builds_it_once() {
        let fake = Fake::new();
        let queue = fake.queue();
        queue.request(&0, Priority::Needed);
        fake.wait_started(0);
        queue.warm([10, 11, 12]);
        assert!(queue.request(&12, Priority::Needed).is_pending());
        assert!(queue.request(&12, Priority::Warm).is_pending());
        fake.open();
        assert!(queue.wait_idle(LONG));
        assert_eq!(*fake.order.lock().unwrap(), vec![0, 12, 10, 11]);
    }

    #[test]
    fn many_threads_asking_for_one_key_build_it_once() {
        let builds = Arc::new(AtomicUsize::new(0));
        let counter = builds.clone();
        let queue = Arc::new(CompileQueue::new(config(4), move |key: &u32| {
            counter.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            Ok(*key * 2)
        }));
        let askers: Vec<_> = (0..8)
            .map(|_| {
                let queue = queue.clone();
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        queue.request(&21, Priority::Needed);
                        queue.request(&22, Priority::Prefetch);
                    }
                    **queue.request_wait(&21, LONG).ready().unwrap()
                })
            })
            .collect();
        for asker in askers {
            assert_eq!(asker.join().unwrap(), 42);
        }
        assert!(queue.wait_idle(LONG));
        assert_eq!(builds.load(Ordering::SeqCst), 2);
        assert_eq!(queue.stats().built, 2);
    }

    #[test]
    fn warm_skips_known_keys_and_counts_progress() {
        let queue = CompileQueue::new(config(2), |key: &u32| Ok(*key));
        assert_eq!(queue.warm([1, 2, 3]), 3);
        assert!(queue.wait_idle(LONG));
        assert_eq!(queue.warm([2, 3, 4]), 1);
        assert!(queue.wait_idle(LONG));
        let stats = queue.stats();
        assert_eq!((stats.warm_total, stats.warm_done), (4, 4));
        assert_eq!(stats.ready, 4);
        assert!(queue.poll(&4).unwrap().ready().is_some());
    }

    #[test]
    fn a_failed_or_panicking_build_is_remembered_until_forgotten() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let counter = attempts.clone();
        let queue = CompileQueue::new(config(1), move |key: &u32| {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            match *key {
                1 => Err("no such shader".into()),
                2 => panic!("builder bug"),
                _ if n > 5 => Ok("later".to_string()),
                _ => Ok("fine".to_string()),
            }
        });
        let Poll::Failed(why) = queue.request_wait(&1, LONG) else {
            panic!("expected a failure");
        };
        assert_eq!(&*why, "no such shader");
        assert!(matches!(queue.request_wait(&2, LONG), Poll::Failed(_)));
        assert!(queue.request_wait(&3, LONG).ready().is_some());
        assert!(matches!(
            queue.request(&1, Priority::Needed),
            Poll::Failed(_)
        ));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert_eq!(queue.stats().failed, 2);
        assert!(queue.forget(&1));
        assert!(queue.request(&1, Priority::Needed).is_pending());
        assert!(queue.wait_idle(LONG));
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn request_wait_gives_up_at_the_timeout_and_the_build_still_lands() {
        let fake = Fake::new();
        let queue = fake.queue();
        assert!(
            queue
                .request_wait(&0, Duration::from_millis(30))
                .is_pending()
        );
        fake.open();
        assert!(queue.request_wait(&0, LONG).ready().is_some());
    }

    #[test]
    fn dropping_the_queue_stops_the_workers() {
        let fake = Fake::new();
        let queue = fake.queue();
        queue.request(&0, Priority::Needed);
        fake.wait_started(0);
        queue.warm(100..200);
        fake.open();
        drop(queue);
        assert!(fake.order.lock().unwrap().len() < 101);
    }

    /// Counts builds running at once and the weight they hold, and holds key 0 until the gate opens when asked.
    #[derive(Default)]
    struct Gauge {
        running: AtomicUsize,
        most: AtomicUsize,
        held: AtomicUsize,
        heaviest: AtomicUsize,
        order: Mutex<Vec<u32>>,
        gate: (Mutex<bool>, Condvar),
    }

    impl Gauge {
        fn enter(&self, key: u32, weight: usize, wait_for_gate: bool) {
            let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(running, Ordering::SeqCst);
            let held = self.held.fetch_add(weight, Ordering::SeqCst) + weight;
            self.heaviest.fetch_max(held, Ordering::SeqCst);
            self.order.lock().unwrap().push(key);
            if wait_for_gate {
                let mut open = self.gate.0.lock().unwrap();
                while !*open {
                    open = self.gate.1.wait(open).unwrap();
                }
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
            self.held.fetch_sub(weight, Ordering::SeqCst);
            self.running.fetch_sub(1, Ordering::SeqCst);
        }

        fn open(&self) {
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
        }

        fn started(&self) -> Vec<u32> {
            self.order.lock().unwrap().clone()
        }

        fn wait_for_a_start(&self) {
            let start = Instant::now();
            while self.started().is_empty() {
                assert!(start.elapsed() < LONG, "builder never started");
                std::thread::yield_now();
            }
        }
    }

    fn settle() {
        std::thread::sleep(Duration::from_millis(60));
    }

    #[test]
    fn no_more_builds_run_at_once_than_there_are_workers() {
        let gauge = Arc::new(Gauge::default());
        let for_builder = gauge.clone();
        let queue = CompileQueue::new(config(3), move |key: &u32| {
            for_builder.enter(*key, 0, false);
            Ok(*key)
        });
        queue.prefetch(0..40);
        assert!(queue.wait_idle(LONG));
        assert!(gauge.most.load(Ordering::SeqCst) <= 3);
        let stats = queue.stats();
        assert!(stats.peak_building <= 3);
        assert_eq!(stats.built, 40);
    }

    fn budgeted(
        workers: usize,
        budget: u64,
        weights: fn(&u32) -> u64,
        gate: bool,
    ) -> (Arc<Gauge>, CompileQueue<u32, u32>) {
        let gauge = Arc::new(Gauge::default());
        let for_builder = gauge.clone();
        let queue = CompileQueue::weighted(
            Config {
                memory_budget: Some(budget),
                ..config(workers)
            },
            weights,
            move |key: &u32| {
                for_builder.enter(*key, weights(key) as usize, gate && *key == 0);
                Ok(*key)
            },
        );
        (gauge, queue)
    }

    #[test]
    fn the_memory_budget_caps_what_builds_hold_together() {
        let (gauge, queue) = budgeted(4, 100, |_| 40, false);
        queue.prefetch(0..30);
        assert!(queue.wait_idle(LONG));
        assert!(gauge.heaviest.load(Ordering::SeqCst) <= 100);
        assert!(gauge.most.load(Ordering::SeqCst) <= 2);
        let stats = queue.stats();
        assert!(stats.peak_memory <= 100 && stats.peak_memory >= 40);
        assert_eq!((stats.built, stats.memory_in_use), (30, 0));
    }

    #[test]
    fn a_build_heavier_than_the_whole_budget_runs_alone() {
        let (gauge, queue) = budgeted(4, 100, |key| if *key == 7 { 500 } else { 10 }, false);
        queue.prefetch(0..20);
        assert!(queue.wait_idle(LONG));
        assert_eq!(queue.stats().built, 20);
        assert!(gauge.started().contains(&7));
        assert_eq!(
            gauge.heaviest.load(Ordering::SeqCst),
            500,
            "something ran beside the heavy build"
        );
    }

    #[test]
    fn a_build_that_does_not_fit_holds_back_everything_behind_it() {
        let (gauge, queue) = budgeted(2, 100, |key| if *key == 10 { 10 } else { 60 }, true);
        queue.request(&0, Priority::Needed);
        gauge.wait_for_a_start();
        queue.request(&1, Priority::Needed);
        queue.warm([10]);
        settle();
        assert_eq!(
            gauge.started(),
            vec![0],
            "1 does not fit and 10 may not pass it"
        );
        gauge.open();
        assert!(queue.wait_idle(LONG));
        assert_eq!(gauge.started(), vec![0, 1, 10]);
    }

    #[test]
    fn a_draw_that_skips_gets_pending_at_once_and_the_build_moves_up() {
        let fake = Fake::new();
        let queue = fake.queue();
        queue.request(&0, Priority::Needed);
        fake.wait_started(0);
        queue.warm([10, 11, 12]);
        let start = Instant::now();
        assert!(queue.for_draw(&12, DrawPolicy::Skip).is_pending());
        assert!(start.elapsed() < Duration::from_millis(500));
        fake.open();
        assert!(queue.wait_idle(LONG));
        assert_eq!(*fake.order.lock().unwrap(), vec![0, 12, 10, 11]);
        assert!(queue.for_draw(&12, DrawPolicy::Skip).ready().is_some());
    }

    #[test]
    fn a_draw_that_waits_waits_for_its_own_build_and_nothing_else() {
        let gauge = Arc::new(Gauge::default());
        let for_builder = gauge.clone();
        let queue = Arc::new(CompileQueue::new(config(2), move |key: &u32| {
            for_builder.enter(*key, 0, *key == 0);
            Ok(*key)
        }));
        queue.request(&0, Priority::Needed);
        gauge.wait_for_a_start();
        let quick = queue.for_draw(&1, DrawPolicy::Wait);
        assert_eq!(**quick.ready().unwrap(), 1);
        assert!(queue.poll(&0).unwrap().is_pending());

        let waiting = {
            let queue = queue.clone();
            std::thread::spawn(move || queue.for_draw(&0, DrawPolicy::Wait))
        };
        settle();
        assert!(!waiting.is_finished());
        gauge.open();
        let done = waiting.join().unwrap();
        assert_eq!(**done.ready().unwrap(), 0);
    }

    #[test]
    fn a_draw_sees_a_failed_build_as_failed_under_either_policy() {
        let queue = CompileQueue::new(config(1), |_: &u32| Err::<u32, _>("no".into()));
        assert!(matches!(
            queue.for_draw(&1, DrawPolicy::Wait),
            Poll::Failed(_)
        ));
        assert!(matches!(
            queue.for_draw(&1, DrawPolicy::Skip),
            Poll::Failed(_)
        ));
    }

    #[test]
    fn the_draw_policy_reads_from_text_and_defaults_to_waiting() {
        assert_eq!(DrawPolicy::default(), DrawPolicy::Wait);
        assert_eq!(" Skip ".parse::<DrawPolicy>(), Ok(DrawPolicy::Skip));
        assert_eq!("wait".parse::<DrawPolicy>(), Ok(DrawPolicy::Wait));
        assert!("later".parse::<DrawPolicy>().is_err());
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp/compile-queue")
            .join(name);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cache_file_round_trips_and_rejects_stale_or_damaged_files() {
        let path = scratch("file").join("pipelines.cache");
        let file = CacheFile::new(&path, 7);
        let _ = std::fs::remove_file(&path);
        assert!(file.load().is_none());
        let blob: Vec<u8> = (0..1000u32).map(|n| (n * 7) as u8).collect();
        file.store(&blob).unwrap();
        assert_eq!(file.load().unwrap(), blob);
        assert!(CacheFile::new(&path, 8).load().is_none());
        let mut bytes = std::fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        assert!(file.load().is_none());
        std::fs::write(&path, b"junk").unwrap();
        assert!(file.load().is_none());
    }

    #[test]
    fn the_cache_is_saved_when_the_queue_goes_idle() {
        let path = scratch("idle").join("pipelines.cache");
        let _ = std::fs::remove_file(&path);
        let file = CacheFile::new(&path, 1);
        let built = Arc::new(AtomicUsize::new(0));
        let for_builder = built.clone();
        let for_snapshot = built.clone();
        let queue = CompileQueue::new(
            Config {
                workers: 2,
                worker_init: None,
                memory_budget: None,
                persist: Some(Persist {
                    file: file.clone(),
                    snapshot: Box::new(move || {
                        Some(vec![for_snapshot.load(Ordering::SeqCst) as u8; 16])
                    }),
                    every: 1000,
                }),
            },
            move |key: &u32| {
                for_builder.fetch_add(1, Ordering::SeqCst);
                Ok(*key)
            },
        );
        queue.warm(0..5);
        assert!(queue.wait_idle(LONG));
        let start = Instant::now();
        while queue.stats().flushes == 0 {
            assert!(start.elapsed() < LONG, "never saved");
            std::thread::yield_now();
        }
        assert_eq!(file.load().unwrap().len(), 16);
        assert_eq!(queue.stats().flush_errors, 0);
        queue.flush().unwrap();
    }
}
