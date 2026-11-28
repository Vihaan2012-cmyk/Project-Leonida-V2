//! Pipeline state objects built off the game's thread.
//!
//! `CreateGraphicsPipelineState` hands the description to [`PipelineService::create_graphics`] and gets a
//! [`PipelineHandle`] back at once; a worker pool translates the shaders and builds the Vulkan pipeline. A draw
//! asks its handle for the pipeline and either waits for that build or skips, by [`DrawPolicy`]. When the service
//! starts it also loads the device's pipeline environment (the pipeline cache file among it) on a thread of its
//! own, and replays the pipelines earlier sessions created from the [`SeenLog`], so the game finds them built.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::seen::{BytecodeStore, Pso, PsoKey, SeenEntry, SeenLog};
use super::{BuildError, CompileQueue, Config, DrawPolicy, Poll, Priority, Stats};
use crate::pipeline::{ComputePsoDesc, GraphicsPsoDesc, Pipeline, PipelineEnv, PipelineError};

/// What the service builds pipelines with: the device's pipeline environment, or a stand-in in tests.
pub trait PipelineBuilder: Send + Sync + 'static {
    type Built: Clone + Send + Sync + 'static;

    fn graphics(&self, desc: &GraphicsPsoDesc) -> Result<Self::Built, PipelineError>;
    fn compute(&self, desc: &ComputePsoDesc) -> Result<Self::Built, PipelineError>;
    /// The blob `GetCachedBlob` returns for the pipeline of the description `key`.
    fn cached_blob(&self, key: &PsoKey) -> Vec<u8>;
    /// Writes the driver's pipeline cache to its file.
    fn save_cache(&self) -> Result<(), PipelineError>;
}

impl PipelineBuilder for PipelineEnv {
    type Built = Arc<Pipeline>;

    fn graphics(&self, desc: &GraphicsPsoDesc) -> Result<Arc<Pipeline>, PipelineError> {
        self.create_graphics(desc)
    }

    fn compute(&self, desc: &ComputePsoDesc) -> Result<Arc<Pipeline>, PipelineError> {
        self.create_compute(desc)
    }

    fn cached_blob(&self, key: &PsoKey) -> Vec<u8> {
        self.cache.cached_blob(key)
    }

    fn save_cache(&self) -> Result<(), PipelineError> {
        self.cache.save()
    }
}

pub struct ServiceConfig {
    /// Threads that translate and build; at least one.
    pub workers: usize,
    /// The most the builds running together may weigh, `None` for no cap. A build weighs `base_weight` plus
    /// `weight_per_byte` for each byte of its shaders: translating and building a large shader takes memory in
    /// proportion to its size, which has not been measured here.
    pub memory_budget: Option<u64>,
    pub base_weight: u64,
    pub weight_per_byte: u64,
    /// What a draw does when its pipeline is not ready.
    pub draw_policy: DrawPolicy,
    /// Where `pipelines-seen` is kept and what holds the shader bytecode its records refer to. Without them
    /// nothing is recorded or replayed.
    pub seen: Option<(PathBuf, Arc<dyn BytecodeStore>)>,
    /// Sessions of pipelines the log remembers.
    pub keep_sessions: u64,
    /// How often the pipeline cache file is written while builds are finishing.
    pub save_every: Duration,
}

impl Default for ServiceConfig {
    fn default() -> ServiceConfig {
        ServiceConfig {
            workers: Config::default().workers,
            memory_budget: Some(4 << 30),
            base_weight: 16 << 20,
            weight_per_byte: 1024,
            draw_policy: DrawPolicy::Wait,
            seen: None,
            keep_sessions: 8,
            save_every: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServiceStats {
    pub queue: Stats,
    /// `create_*` calls, and those whose pipeline an earlier call or the replay had already built or queued.
    pub created: u64,
    pub already_known: u64,
    /// Draws that found their pipeline not ready: those that waited, those skipped, and the time waiting.
    pub draws_waited: u64,
    pub draws_skipped: u64,
    /// Draws dropped because their pipeline failed to build.
    pub draws_failed: u64,
    pub wait_micros: u64,
    pub recorded: u64,
    pub record_errors: u64,
    pub cache_saves: u64,
    /// Pipelines the log named at start.
    pub replayed: usize,
}

#[derive(Default)]
struct Counters {
    created: AtomicU64,
    already_known: AtomicU64,
    draws_waited: AtomicU64,
    draws_skipped: AtomicU64,
    draws_failed: AtomicU64,
    wait_micros: AtomicU64,
    recorded: AtomicU64,
    record_errors: AtomicU64,
    cache_saves: AtomicU64,
    replayed: AtomicU64,
}

#[derive(Clone)]
enum Wanted {
    Loaded(Arc<Pso>),
    /// Named by the log; its bytecode is read when a worker comes to build it.
    Seen(SeenEntry),
}

struct Environment<B> {
    state: Mutex<Option<Result<Arc<B>, String>>>,
    ready: Condvar,
}

impl<B> Environment<B> {
    fn set(&self, result: Result<Arc<B>, String>) {
        *self.state.lock().expect("environment") = Some(result);
        self.ready.notify_all();
    }

    fn wait(&self) -> Result<Arc<B>, String> {
        let mut state = self.state.lock().expect("environment");
        loop {
            if let Some(result) = &*state {
                return result.clone();
            }
            state = self.ready.wait(state).expect("environment");
        }
    }

    fn get(&self) -> Option<Arc<B>> {
        self.state
            .lock()
            .expect("environment")
            .as_ref()
            .and_then(|r| r.as_ref().ok().cloned())
    }
}

struct Shared<B: PipelineBuilder> {
    env: Environment<B>,
    descs: Mutex<HashMap<PsoKey, Wanted>>,
    log: OnceLock<Arc<SeenLog>>,
    counters: Counters,
    base_weight: u64,
    weight_per_byte: u64,
}

impl<B: PipelineBuilder> Shared<B> {
    fn weight(&self, key: &PsoKey) -> u64 {
        let bytes = match self.descs.lock().expect("descriptions").get(key) {
            Some(Wanted::Loaded(pso)) => u64::from(pso.bytecode_len()),
            Some(Wanted::Seen(entry)) => u64::from(entry.bytecode_len()),
            None => 0,
        };
        self.base_weight + bytes * self.weight_per_byte
    }

    /// Forgets the descriptions of pipelines that are done: one a `create` call stored just as its build ended
    /// would otherwise stay.
    fn sweep(&self, queue: &CompileQueue<PsoKey, B::Built>) {
        let keys: Vec<PsoKey> = self
            .descs
            .lock()
            .expect("descriptions")
            .keys()
            .copied()
            .collect();
        let done: Vec<PsoKey> = keys
            .into_iter()
            .filter(|key| queue.poll(key).is_some_and(|p| !p.is_pending()))
            .collect();
        let mut descs = self.descs.lock().expect("descriptions");
        for key in done {
            descs.remove(&key);
        }
    }

    fn build(&self, key: &PsoKey) -> Result<B::Built, BuildError> {
        let env = self
            .env
            .wait()
            .map_err(|e| BuildError(format!("the pipeline environment is not available: {e}")))?;
        let wanted = self
            .descs
            .lock()
            .expect("descriptions")
            .get(key)
            .cloned()
            .ok_or_else(|| BuildError::from("no description for the pipeline"))?;
        let built = self.build_from(&env, wanted);
        self.descs.lock().expect("descriptions").remove(key);
        built
    }

    fn build_from(&self, env: &B, wanted: Wanted) -> Result<B::Built, BuildError> {
        let pso = match wanted {
            Wanted::Loaded(pso) => pso,
            Wanted::Seen(entry) => {
                let log = self
                    .log
                    .get()
                    .ok_or_else(|| BuildError::from("no log to read the pipeline from"))?;
                Arc::new(
                    entry
                        .load(&**log.bytecode())
                        .map_err(|e| BuildError(e.to_string()))?,
                )
            }
        };
        match &*pso {
            Pso::Graphics(desc) => env.graphics(desc),
            Pso::Compute(desc) => env.compute(desc),
        }
        .map_err(|e| BuildError(e.to_string()))
    }
}

struct Inner<B: PipelineBuilder> {
    shared: Arc<Shared<B>>,
    queue: Arc<CompileQueue<PsoKey, B::Built>>,
    skip: AtomicBool,
    seen: Mutex<Option<mpsc::Sender<Arc<Pso>>>>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl<B: PipelineBuilder> Inner<B> {
    fn policy(&self) -> DrawPolicy {
        if self.skip.load(Ordering::Relaxed) {
            DrawPolicy::Skip
        } else {
            DrawPolicy::Wait
        }
    }

    fn stop_threads(&self) {
        self.seen.lock().expect("seen channel").take();
        for thread in self.threads.lock().expect("threads").drain(..) {
            let _ = thread.join();
        }
    }
}

impl<B: PipelineBuilder> Drop for Inner<B> {
    fn drop(&mut self) {
        self.stop_threads();
    }
}

/// A pipeline the game created, to be asked for when a draw needs it.
pub struct PipelineHandle<B: PipelineBuilder> {
    key: PsoKey,
    inner: Arc<Inner<B>>,
}

impl<B: PipelineBuilder> Clone for PipelineHandle<B> {
    fn clone(&self) -> Self {
        PipelineHandle {
            key: self.key,
            inner: self.inner.clone(),
        }
    }
}

/// What a draw gets from its handle.
pub enum Resolved<T> {
    Ready(T),
    /// Still building: the draw is skipped this frame.
    NotReady,
    /// The pipeline could not be built; the reason is kept.
    Failed(Arc<str>),
}

impl<T> Resolved<T> {
    pub fn ready(self) -> Option<T> {
        match self {
            Resolved::Ready(built) => Some(built),
            _ => None,
        }
    }
}

impl<B: PipelineBuilder> PipelineHandle<B> {
    /// Equal for descriptions that build the same pipeline.
    pub fn key(&self) -> &PsoKey {
        &self.key
    }

    /// The pipeline for a draw, by the configured [`DrawPolicy`].
    pub fn resolve(&self) -> Resolved<B::Built> {
        self.resolve_with(self.inner.policy())
    }

    pub fn resolve_with(&self, policy: DrawPolicy) -> Resolved<B::Built> {
        let queue = &self.inner.queue;
        let counters = &self.inner.shared.counters;
        if let Some(known) = queue.poll(&self.key)
            && !known.is_pending()
        {
            if matches!(known, Poll::Failed(_)) {
                counters.draws_failed.fetch_add(1, Ordering::Relaxed);
            }
            return Self::answer(known);
        }
        match policy {
            DrawPolicy::Skip => {
                counters.draws_skipped.fetch_add(1, Ordering::Relaxed);
                Self::answer(queue.for_draw(&self.key, DrawPolicy::Skip))
            }
            DrawPolicy::Wait => {
                let started = Instant::now();
                let answer = queue.for_draw(&self.key, DrawPolicy::Wait);
                counters.draws_waited.fetch_add(1, Ordering::Relaxed);
                counters
                    .wait_micros
                    .fetch_add(started.elapsed().as_micros() as u64, Ordering::Relaxed);
                Self::answer(answer)
            }
        }
    }

    fn answer(poll: Poll<B::Built>) -> Resolved<B::Built> {
        match poll {
            Poll::Ready(built) => Resolved::Ready((*built).clone()),
            Poll::Pending => Resolved::NotReady,
            Poll::Failed(why) => Resolved::Failed(why),
        }
    }

    /// The pipeline if it is built, without asking for it to be built sooner.
    pub fn ready(&self) -> Option<B::Built> {
        match self.inner.queue.poll(&self.key)? {
            Poll::Ready(built) => Some((*built).clone()),
            _ => None,
        }
    }

    /// The blob `GetCachedBlob` returns: it names the device, driver and description, not the pipeline, so it
    /// does not wait for the build, only for the environment.
    pub fn cached_blob(&self) -> Option<Vec<u8>> {
        let env = self.inner.shared.env.wait().ok()?;
        Some(env.cached_blob(&self.key))
    }
}

pub struct PipelineService<B: PipelineBuilder> {
    inner: Arc<Inner<B>>,
}

impl<B: PipelineBuilder> PipelineService<B> {
    /// Starts the service. `load` makes the builder on a thread of its own: open the pipeline cache file, the
    /// shader store and everything else slow there, and `create_*` calls in the meantime only queue.
    pub fn start(
        config: ServiceConfig,
        load: impl FnOnce() -> Result<B, PipelineError> + Send + 'static,
    ) -> PipelineService<B> {
        let shared = Arc::new(Shared {
            env: Environment {
                state: Mutex::new(None),
                ready: Condvar::new(),
            },
            descs: Mutex::new(HashMap::new()),
            log: OnceLock::new(),
            counters: Counters::default(),
            base_weight: config.base_weight,
            weight_per_byte: config.weight_per_byte,
        });
        let weighing = shared.clone();
        let building = shared.clone();
        let queue = Arc::new(CompileQueue::weighted(
            Config {
                workers: config.workers,
                worker_init: None,
                persist: None,
                memory_budget: config.memory_budget,
            },
            move |key: &PsoKey| weighing.weight(key),
            move |key: &PsoKey| building.build(key),
        ));
        let mut threads = Vec::new();

        let loading = shared.clone();
        threads.push(
            std::thread::Builder::new()
                .name("leonida-pipeline-env".into())
                .spawn(move || {
                    let result = load().map(Arc::new).map_err(|e| e.to_string());
                    loading.env.set(result);
                })
                .expect("spawning the environment thread"),
        );

        let (sender, receiver) = mpsc::channel::<Arc<Pso>>();
        let seen = config.seen.is_some().then_some(sender);
        if let Some((path, bytecode)) = config.seen {
            let shared = shared.clone();
            let queue = queue.clone();
            let keep = config.keep_sessions;
            let every = config.save_every;
            threads.push(
                std::thread::Builder::new()
                    .name("leonida-pipeline-seen".into())
                    .spawn(move || {
                        seen_thread(&shared, &queue, &path, bytecode, keep, every, &receiver)
                    })
                    .expect("spawning the log thread"),
            );
        }
        PipelineService {
            inner: Arc::new(Inner {
                shared,
                queue,
                skip: AtomicBool::new(config.draw_policy == DrawPolicy::Skip),
                seen: Mutex::new(seen),
                threads: Mutex::new(threads),
            }),
        }
    }

    pub fn create_graphics(&self, desc: GraphicsPsoDesc) -> PipelineHandle<B> {
        self.create(Pso::Graphics(Box::new(desc)))
    }

    pub fn create_compute(&self, desc: ComputePsoDesc) -> PipelineHandle<B> {
        self.create(Pso::Compute(desc))
    }

    /// Queues the build and returns. A description the service already has built or queued (from an earlier call
    /// or the replay) is not built again.
    pub fn create(&self, pso: Pso) -> PipelineHandle<B> {
        let inner = &self.inner;
        let shared = &inner.shared;
        shared.counters.created.fetch_add(1, Ordering::Relaxed);
        let key = pso.key();
        let pso = Arc::new(pso);
        let known = inner.queue.poll(&key);
        match known {
            Some(Poll::Ready(_) | Poll::Failed(_)) => {
                shared
                    .counters
                    .already_known
                    .fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                if known.is_some() {
                    shared
                        .counters
                        .already_known
                        .fetch_add(1, Ordering::Relaxed);
                }
                shared
                    .descs
                    .lock()
                    .expect("descriptions")
                    .insert(key, Wanted::Loaded(pso.clone()));
                let after = inner.queue.request(&key, Priority::Prefetch);
                if !after.is_pending() {
                    shared.descs.lock().expect("descriptions").remove(&key);
                }
            }
        }
        if let Some(sender) = &*inner.seen.lock().expect("seen channel") {
            let _ = sender.send(pso);
        }
        PipelineHandle {
            key,
            inner: inner.clone(),
        }
    }

    pub fn stats(&self) -> ServiceStats {
        let c = &self.inner.shared.counters;
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        ServiceStats {
            queue: self.inner.queue.stats(),
            created: load(&c.created),
            already_known: load(&c.already_known),
            draws_waited: load(&c.draws_waited),
            draws_skipped: load(&c.draws_skipped),
            draws_failed: load(&c.draws_failed),
            wait_micros: load(&c.wait_micros),
            recorded: load(&c.recorded),
            record_errors: load(&c.record_errors),
            cache_saves: load(&c.cache_saves),
            replayed: load(&c.replayed) as usize,
        }
    }

    /// What a draw does when its pipeline is not ready.
    pub fn draw_policy(&self) -> DrawPolicy {
        self.inner.policy()
    }

    /// Changes the policy for draws from now on, handles that already exist included.
    pub fn set_draw_policy(&self, policy: DrawPolicy) {
        self.inner
            .skip
            .store(policy == DrawPolicy::Skip, Ordering::Relaxed);
    }

    /// Stops recording and replaying: the log is flushed and the pipeline cache written if builds finished since
    /// it last was, and the log and environment threads end. Pipelines already created stay usable and builds
    /// already queued still run; what is created later is built but no longer recorded.
    pub fn shutdown(&self) {
        self.inner.stop_threads();
    }

    /// Waits until the environment is loaded; false if it could not be.
    pub fn wait_environment(&self) -> Result<(), String> {
        self.inner.shared.env.wait().map(|_| ())
    }

    /// Waits until nothing is queued or building, up to `timeout`.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        self.inner.queue.wait_idle(timeout)
    }

    /// Writes the pipeline cache now, if the environment is loaded.
    pub fn save_cache(&self) -> Result<(), PipelineError> {
        match self.inner.shared.env.get() {
            Some(env) => {
                env.save_cache()?;
                self.inner
                    .shared
                    .counters
                    .cache_saves
                    .fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// The session the log is writing, once it has been opened.
    pub fn session(&self) -> Option<u64> {
        self.inner.shared.log.get().map(|log| log.session())
    }
}

/// Opens the log, queues what earlier sessions created, then writes what this session creates and saves the
/// pipeline cache now and then, until the service is dropped.
fn seen_thread<B: PipelineBuilder>(
    shared: &Shared<B>,
    queue: &CompileQueue<PsoKey, B::Built>,
    path: &std::path::Path,
    bytecode: Arc<dyn BytecodeStore>,
    keep: u64,
    save_every: Duration,
    receiver: &mpsc::Receiver<Arc<Pso>>,
) {
    let log = match SeenLog::open(path, bytecode, keep) {
        Ok(log) => Arc::new(log),
        Err(_) => {
            shared
                .counters
                .record_errors
                .fetch_add(1, Ordering::Relaxed);
            for _ in receiver {}
            return;
        }
    };
    let _ = shared.log.set(log.clone());
    let warm = log.warm_list();
    {
        let mut descs = shared.descs.lock().expect("descriptions");
        for entry in warm {
            descs
                .entry(entry.key)
                .or_insert_with(|| Wanted::Seen(entry.clone()));
        }
    }
    queue.warm(warm.iter().map(|entry| entry.key));
    shared
        .counters
        .replayed
        .store(warm.len() as u64, Ordering::Relaxed);

    let mut saved_at = (Instant::now(), queue.stats().built);
    let mut swept_at = 0;
    loop {
        match receiver.recv_timeout(
            save_every
                .min(Duration::from_secs(5))
                .max(Duration::from_millis(10)),
        ) {
            Ok(pso) => match log.record(&pso) {
                Ok(true) => {
                    shared.counters.recorded.fetch_add(1, Ordering::Relaxed);
                }
                Ok(false) => {}
                Err(_) => {
                    shared
                        .counters
                        .record_errors
                        .fetch_add(1, Ordering::Relaxed);
                }
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let built = queue.stats().built;
        if built != swept_at {
            swept_at = built;
            shared.sweep(queue);
        }
        if built != saved_at.1 && saved_at.0.elapsed() >= save_every {
            if let Some(env) = shared.env.get()
                && env.save_cache().is_ok()
            {
                shared.counters.cache_saves.fetch_add(1, Ordering::Relaxed);
            }
            saved_at = (Instant::now(), built);
        }
    }
    let _ = log.flush();
    if queue.stats().built != saved_at.1
        && let Some(env) = shared.env.get()
        && env.save_cache().is_ok()
    {
        shared.counters.cache_saves.fetch_add(1, Ordering::Relaxed);
    }
}
