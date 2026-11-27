//! Pipeline compilation off the render thread, with the persistent cache warmed at start.
//!
//! The render thread asks for a pipeline by key with [`CompileQueue::request`] and gets one of three answers
//! at once: ready, still building (skip the draw this frame), or failed. A pool of workers builds through a
//! callback; a key is built once however many callers ask, and what is needed now goes ahead of prefetching,
//! which goes ahead of the start-up warm list.

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
}

impl Default for Config {
    /// Leaves two processors to the game's own threads, and at least one worker.
    fn default() -> Config {
        let cpus = std::thread::available_parallelism().map_or(2, |n| n.get());
        Config {
            workers: cpus.saturating_sub(2).clamp(1, 8),
            worker_init: None,
            persist: None,
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

struct Shared<K, P> {
    inner: Mutex<Inner<K, P>>,
    work: Condvar,
    done: Condvar,
    builder: Box<Builder<K, P>>,
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
            let key = loop {
                if inner.shutdown {
                    return;
                }
                match inner.heap.pop() {
                    Some(job) => {
                        if matches!(inner.slots.get(&job.key), Some(Slot::Queued(p)) if *p == job.priority)
                        {
                            inner.slots.insert(job.key.clone(), Slot::Building);
                            inner.stats.queued -= 1;
                            inner.stats.building += 1;
                            break job.key;
                        }
                    }
                    None => inner = self.work.wait(inner).unwrap_or_else(|e| e.into_inner()),
                }
            };
            drop(inner);

            let result = catch_unwind(AssertUnwindSafe(|| (self.builder)(&key)));

            let mut inner = self.lock();
            inner.stats.building -= 1;
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
        let first = self.request(key, Priority::Needed);
        if !first.is_pending() {
            return first;
        }
        let deadline = Instant::now() + timeout;
        let mut inner = self.shared.lock();
        loop {
            match inner.slots.get(key) {
                Some(Slot::Ready(p)) => return Poll::Ready(p.clone()),
                Some(Slot::Failed(e)) => return Poll::Failed(e.clone()),
                _ => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Poll::Pending;
            }
            inner = self
                .shared
                .done
                .wait_timeout(inner, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
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
