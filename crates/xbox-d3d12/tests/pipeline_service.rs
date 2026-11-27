//! The pipeline service and the seen log, with a builder that stands in for the GPU.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use xbox_d3d12::compile_queue::{
    BytecodeStore, DrawPolicy, MemoryBytecode, PipelineBuilder, PipelineService, Pso, PsoKey,
    Resolved, SeenError, SeenLog, ServiceConfig,
};
use xbox_d3d12::pipeline::state::InputElement;
use xbox_d3d12::pipeline::{Bytecode, ComputePsoDesc, GraphicsPsoDesc, PipelineError};
use xbox_d3d12::root::{ParameterKind, RootParameter, RootSignature, RootVersion, Visibility};

const LONG: Duration = Duration::from_secs(20);

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/test-tmp/pipeline-service")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn root() -> Arc<RootSignature> {
    Arc::new(RootSignature {
        version: RootVersion::V1_1,
        parameters: vec![RootParameter {
            kind: ParameterKind::Uav {
                register: 0,
                space: 0,
                flags: 0,
            },
            visibility: Visibility::All,
        }],
        static_samplers: Vec::new(),
        flags: 0,
    })
}

/// A graphics description whose shaders are `seed` repeated, so each seed is its own pipeline.
fn graphics(seed: u8) -> Pso {
    let mut desc = GraphicsPsoDesc::new(root());
    desc.vs = Bytecode(vec![seed; 40]);
    desc.ps = Bytecode(vec![seed.wrapping_add(100); 24]);
    desc.render_targets = vec![28, 0, 10];
    desc.depth_format = 40;
    desc.blend.alpha_to_coverage = true;
    desc.blend.targets[1].blend_enable = true;
    desc.blend.targets[1].src = 5;
    desc.raster.depth_bias = -3;
    desc.raster.slope_scaled_bias = 1.5;
    desc.depth_stencil.stencil_enable = true;
    desc.depth_stencil.back.func = 3;
    desc.input_layout = vec![InputElement {
        semantic: "POSITION".into(),
        semantic_index: 0,
        format: 6,
        slot: 1,
        offset: 0xFFFF_FFFF,
        class: 1,
        step_rate: 2,
    }];
    Pso::Graphics(Box::new(desc))
}

fn compute(seed: u8) -> Pso {
    Pso::Compute(ComputePsoDesc {
        root: None,
        cs: Bytecode(vec![seed; 64]),
    })
}

fn memory() -> Arc<MemoryBytecode> {
    Arc::new(MemoryBytecode::default())
}

fn open(path: &Path, store: &Arc<MemoryBytecode>, keep: u64) -> SeenLog {
    SeenLog::open(path, store.clone(), keep).unwrap()
}

fn warm_keys(log: &SeenLog) -> Vec<PsoKey> {
    log.warm_list().iter().map(|e| e.key).collect()
}

#[test]
fn a_recorded_pipeline_reads_back_the_same_and_the_bytecode_is_kept_once() {
    let path = scratch("round-trip").join("pipelines-seen");
    let store = memory();
    let (first, second, third) = (graphics(1), compute(2), graphics(1));
    {
        let log = open(&path, &store, 8);
        assert_eq!(log.session(), 1);
        assert!(log.warm_list().is_empty());
        assert!(log.record(&first).unwrap());
        assert!(log.record(&second).unwrap());
        assert!(!log.record(&third).unwrap(), "once per session");
    }
    assert_eq!(store.len(), 3);
    let log = open(&path, &store, 8);
    assert_eq!(log.session(), 2);
    assert_eq!(warm_keys(&log), vec![first.key(), second.key()]);
    for (entry, original) in log.warm_list().iter().zip([&first, &second]) {
        let loaded = entry.load(&*store).unwrap();
        assert_eq!(loaded.key(), original.key());
        assert_eq!(entry.bytecode_len(), original.bytecode_len());
    }
    let Pso::Graphics(desc) = log.warm_list()[0].load(&*store).unwrap() else {
        panic!("a graphics description");
    };
    assert_eq!(desc.render_targets, vec![28, 0, 10]);
    assert_eq!(desc.input_layout[0].offset, 0xFFFF_FFFF);
    assert_eq!(desc.raster.depth_bias, -3);
    assert_eq!(desc.root.serialize(), root().serialize());
}

#[test]
fn the_warm_list_starts_with_the_latest_session_in_creation_order() {
    let path = scratch("order").join("pipelines-seen");
    let store = memory();
    let (a, b, c, d) = (graphics(1), graphics(2), graphics(3), graphics(4));
    for session in [vec![&a, &b, &c], vec![&c, &a], vec![&d]] {
        let log = open(&path, &store, 8);
        for pso in session {
            log.record(pso).unwrap();
        }
    }
    let log = open(&path, &store, 8);
    assert_eq!(log.session(), 4);
    assert_eq!(
        warm_keys(&log),
        vec![d.key(), c.key(), a.key(), b.key()],
        "session 3, then session 2 in its order, then what only session 1 made"
    );
}

#[test]
fn sessions_past_the_retention_are_forgotten() {
    let path = scratch("retention").join("pipelines-seen");
    let store = memory();
    let (a, b, c) = (graphics(1), graphics(2), graphics(3));
    for pso in [&a, &b, &c] {
        let log = open(&path, &store, 2);
        log.record(pso).unwrap();
    }
    let log = open(&path, &store, 2);
    assert_eq!(warm_keys(&log), vec![c.key(), b.key()]);
}

#[test]
fn a_torn_last_record_is_dropped_and_later_records_survive() {
    let path = scratch("torn").join("pipelines-seen");
    let store = memory();
    let (a, b, c) = (graphics(1), graphics(2), graphics(3));
    {
        let log = open(&path, &store, 8);
        log.record(&a).unwrap();
        log.record(&b).unwrap();
    }
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() - 5]).unwrap();
    {
        let log = open(&path, &store, 8);
        assert_eq!(warm_keys(&log), vec![a.key()]);
        log.record(&c).unwrap();
    }
    let log = open(&path, &store, 8);
    assert_eq!(warm_keys(&log), vec![c.key(), a.key()]);
}

#[test]
fn a_file_that_is_not_a_log_is_an_empty_log() {
    let path = scratch("junk").join("pipelines-seen");
    std::fs::write(&path, b"not a log at all").unwrap();
    let store = memory();
    let log = open(&path, &store, 8);
    assert!(log.warm_list().is_empty());
    log.record(&graphics(1)).unwrap();
    drop(log);
    assert_eq!(open(&path, &store, 8).warm_list().len(), 1);
}

#[test]
fn a_pipeline_whose_bytecode_is_gone_says_so() {
    let path = scratch("missing").join("pipelines-seen");
    let store = memory();
    {
        let log = open(&path, &store, 8);
        log.record(&graphics(1)).unwrap();
    }
    let log = open(&path, &store, 8);
    let empty = MemoryBytecode::default();
    assert!(matches!(
        log.warm_list()[0].load(&empty),
        Err(SeenError::MissingBytecode(_))
    ));
}

// the service

#[derive(Default)]
struct Machine {
    built: Mutex<Vec<u8>>,
    running: AtomicUsize,
    most: AtomicUsize,
    gate: Mutex<bool>,
    opened: Condvar,
    closed: bool,
}

#[derive(Clone)]
struct Fake(Arc<Machine>);

impl Fake {
    fn new(closed: bool) -> Fake {
        Fake(Arc::new(Machine {
            closed,
            ..Machine::default()
        }))
    }

    fn open(&self) {
        *self.0.gate.lock().unwrap() = true;
        self.0.opened.notify_all();
    }

    fn built(&self) -> Vec<u8> {
        self.0.built.lock().unwrap().clone()
    }

    fn build(&self, first: u8) -> Result<u8, PipelineError> {
        let running = self.0.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.0.most.fetch_max(running, Ordering::SeqCst);
        self.0.built.lock().unwrap().push(first);
        if self.0.closed {
            let mut open = self.0.gate.lock().unwrap();
            while !*open {
                open = self.0.opened.wait(open).unwrap();
            }
        }
        self.0.running.fetch_sub(1, Ordering::SeqCst);
        if first == 99 {
            return Err(PipelineError::Invalid("a pipeline that cannot be built"));
        }
        Ok(first)
    }
}

impl PipelineBuilder for Fake {
    type Built = u8;

    fn graphics(&self, desc: &GraphicsPsoDesc) -> Result<u8, PipelineError> {
        self.build(desc.vs.0[0])
    }

    fn compute(&self, desc: &ComputePsoDesc) -> Result<u8, PipelineError> {
        self.build(desc.cs.0[0])
    }

    fn cached_blob(&self, key: &PsoKey) -> Vec<u8> {
        key.to_vec()
    }

    fn save_cache(&self) -> Result<(), PipelineError> {
        Ok(())
    }
}

fn config(workers: usize, seen: Option<(&Path, &Arc<MemoryBytecode>)>) -> ServiceConfig {
    ServiceConfig {
        workers,
        memory_budget: None,
        seen: seen
            .map(|(path, store)| (path.to_path_buf(), store.clone() as Arc<dyn BytecodeStore>)),
        save_every: Duration::from_millis(20),
        ..ServiceConfig::default()
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < LONG, "never happened: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn creating_a_pipeline_returns_before_its_build_and_a_draw_waits_for_it() {
    let fake = Fake::new(true);
    let machine = fake.clone();
    let service = Arc::new(PipelineService::start(config(2, None), move || Ok(machine)));
    let started = Instant::now();
    let handle = service.create(graphics(7));
    assert!(started.elapsed() < Duration::from_millis(250));
    wait_for("the build to start", || fake.built() == vec![7]);
    assert!(handle.ready().is_none());

    let drawing = {
        let handle = handle.clone();
        std::thread::spawn(move || handle.resolve())
    };
    std::thread::sleep(Duration::from_millis(50));
    assert!(!drawing.is_finished(), "the default policy waits");
    fake.open();
    assert_eq!(drawing.join().unwrap().ready(), Some(7));
    assert_eq!(service.stats().draws_waited, 1);
    assert_eq!(handle.cached_blob().unwrap(), handle.key().to_vec());
}

#[test]
fn a_draw_that_skips_is_told_not_ready_and_the_build_is_asked_for_sooner() {
    let fake = Fake::new(true);
    let machine = fake.clone();
    let service = PipelineService::start(config(1, None), move || Ok(machine));
    let first = service.create(graphics(1));
    wait_for("the first build", || fake.built() == vec![1]);
    let later: Vec<_> = (2..=4).map(|n| service.create(graphics(n))).collect();
    assert!(matches!(
        later[2].resolve_with(DrawPolicy::Skip),
        Resolved::NotReady
    ));
    fake.open();
    assert!(service.wait_idle(LONG));
    assert_eq!(
        fake.built(),
        vec![1, 4, 2, 3],
        "the one a draw wanted went first"
    );
    assert_eq!(service.stats().draws_skipped, 1);
    assert_eq!(first.resolve_with(DrawPolicy::Skip).ready(), Some(1));
}

#[test]
fn a_pipeline_that_cannot_be_built_is_reported_to_the_draw() {
    let service = PipelineService::start(config(1, None), || Ok(Fake::new(false)));
    let handle = service.create(compute(99));
    let Resolved::Failed(why) = handle.resolve() else {
        panic!("expected a failure");
    };
    assert!(why.contains("cannot be built"), "{why}");
}

#[test]
fn builds_never_exceed_the_workers_and_the_memory_budget() {
    let fake = Fake::new(false);
    let machine = fake.clone();
    let mut cfg = config(2, None);
    cfg.base_weight = 10;
    cfg.weight_per_byte = 0;
    cfg.memory_budget = Some(10);
    let service = PipelineService::start(cfg, move || Ok(machine));
    for seed in 0..20 {
        service.create(graphics(seed));
    }
    assert!(service.wait_idle(LONG));
    assert_eq!(fake.built().len(), 20);
    assert_eq!(fake.0.most.load(Ordering::SeqCst), 1, "the budget fits one");
}

#[test]
fn the_next_session_builds_what_the_last_created_before_the_game_asks() {
    let path = scratch("service").join("pipelines-seen");
    let store = memory();
    let (a, b, c) = (graphics(1), compute(2), graphics(3));
    {
        let service =
            PipelineService::start(config(1, Some((&path, &store))), || Ok(Fake::new(false)));
        for pso in [&a, &b, &c] {
            service.create(pso.clone());
        }
        assert!(service.wait_idle(LONG));
        wait_for("the log to take all three", || {
            service.stats().recorded == 3
        });
    }

    let fake = Fake::new(false);
    let machine = fake.clone();
    let service = PipelineService::start(config(1, Some((&path, &store))), move || Ok(machine));
    wait_for("the replay to be queued", || service.stats().replayed == 3);
    assert!(service.wait_idle(LONG));
    assert_eq!(
        fake.built(),
        vec![1, 2, 3],
        "in the order they were created"
    );
    assert_eq!(service.session(), Some(2));

    let before = fake.built().len();
    let handle = service.create(b.clone());
    assert_eq!(handle.ready(), Some(2), "already built, no wait");
    assert_eq!(fake.built().len(), before, "and not built twice");
    assert_eq!(service.stats().already_known, 1);
}

#[test]
fn a_pipeline_the_game_creates_while_the_replay_has_it_queued_is_built_once() {
    let path = scratch("overlap").join("pipelines-seen");
    let store = memory();
    {
        let service =
            PipelineService::start(config(1, Some((&path, &store))), || Ok(Fake::new(false)));
        for seed in 1..=3 {
            service.create(graphics(seed));
        }
        assert!(service.wait_idle(LONG));
        wait_for("the log", || service.stats().recorded == 3);
    }
    let fake = Fake::new(true);
    let machine = fake.clone();
    let service = PipelineService::start(config(1, Some((&path, &store))), move || Ok(machine));
    wait_for("the replay", || service.stats().replayed == 3);
    wait_for("the first build", || fake.built() == vec![1]);
    let handle = service.create(graphics(3));
    fake.open();
    assert_eq!(handle.resolve().ready(), Some(3));
    assert!(service.wait_idle(LONG));
    assert_eq!(fake.built(), vec![1, 3, 2]);
}
