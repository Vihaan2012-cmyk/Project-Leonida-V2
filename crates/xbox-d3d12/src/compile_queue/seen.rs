//! The log of pipelines the game created, kept between sessions so the next start can build them before the
//! game asks.
//!
//! `Output/pipelines-seen` holds one record per pipeline description per session, in creation order, each sealed
//! by a checksum so a record torn by a crash is dropped. A record carries the description's state and root
//! signature and, for each shader, the SHA-256 and length of its bytecode; the bytes themselves live in a
//! [`BytecodeStore`] (the shader database in the game), which keeps one copy however many pipelines use them.
//!
//! [`SeenLog::warm_list`] orders what to build at start: the pipelines of the latest session that has them, in the
//! order the game created them, then those of the session before, and so on. Sessions older than the
//! retention are dropped when the file is next written.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::pipeline::state::{
    BlendState, DepthStencilState, InputElement, RasterizerState, RenderTargetBlend, StencilFace,
};
use crate::pipeline::{Bytecode, ComputePsoDesc, GraphicsPsoDesc};
use crate::root::{RootError, RootSignature};

const MAGIC: [u8; 4] = *b"LPSN";
const VERSION: u32 = 1;
const HEADER: usize = 8;
const SESSION: u8 = 1;
const PIPELINE: u8 = 2;
const GRAPHICS: u8 = 1;
const COMPUTE: u8 = 2;

pub type PsoKey = [u8; 32];

#[derive(Debug, Error)]
pub enum SeenError {
    #[error("pipelines-seen file {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("a recorded pipeline is damaged: {0}")]
    Corrupt(&'static str),
    #[error("a recorded pipeline has a root signature that does not parse: {0}")]
    Root(#[from] RootError),
    #[error("the bytecode {} of a recorded pipeline is no longer stored", hex(.0))]
    MissingBytecode(PsoKey),
}

fn hex(hash: &PsoKey) -> String {
    hash[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// Where shader bytecode is kept by the hash of its contents.
pub trait BytecodeStore: Send + Sync {
    /// Keeps `bytes` under `hash` (their SHA-256). Keeping the same bytes again is harmless.
    fn put(&self, hash: &PsoKey, bytes: &[u8]);
    fn get(&self, hash: &PsoKey) -> Option<Vec<u8>>;
}

/// A [`BytecodeStore`] in memory, for tests and for runs without a shader database.
#[derive(Default)]
pub struct MemoryBytecode(Mutex<HashMap<PsoKey, Vec<u8>>>);

impl MemoryBytecode {
    pub fn len(&self) -> usize {
        self.0.lock().expect("bytecode map").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl BytecodeStore for MemoryBytecode {
    fn put(&self, hash: &PsoKey, bytes: &[u8]) {
        self.0
            .lock()
            .expect("bytecode map")
            .insert(*hash, bytes.to_vec());
    }

    fn get(&self, hash: &PsoKey) -> Option<Vec<u8>> {
        self.0.lock().expect("bytecode map").get(hash).cloned()
    }
}

/// A pipeline description of either kind.
#[derive(Debug, Clone)]
pub enum Pso {
    Graphics(Box<GraphicsPsoDesc>),
    Compute(ComputePsoDesc),
}

impl Pso {
    /// Equal for descriptions that build the same pipeline.
    pub fn key(&self) -> PsoKey {
        match self {
            Pso::Graphics(desc) => desc.key(),
            Pso::Compute(desc) => {
                let mut hash = Sha256::new();
                hash.update(b"compute pipeline");
                match &desc.root {
                    Some(root) => hash.update(root.fingerprint()),
                    None => hash.update([0u8; 32]),
                }
                hash.update(&desc.cs.0);
                hash.finalize().into()
            }
        }
    }

    /// The bytes of all the description's shaders together.
    pub fn bytecode_len(&self) -> u32 {
        self.bytecodes().iter().map(|c| c.0.len() as u32).sum()
    }

    /// The bytes of every shader of the description.
    pub fn bytecodes(&self) -> Vec<&Bytecode> {
        match self {
            Pso::Graphics(desc) => vec![&desc.vs, &desc.ps, &desc.gs, &desc.ms],
            Pso::Compute(desc) => vec![&desc.cs],
        }
    }
}

// encoding

#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }
    fn bool(&mut self, value: bool) {
        self.0.push(u8::from(value));
    }
    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn i32(&mut self, value: i32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn f32(&mut self, value: f32) {
        self.0.extend_from_slice(&value.to_bits().to_le_bytes());
    }
    fn bytes(&mut self, bytes: &[u8]) {
        self.u32(bytes.len() as u32);
        self.0.extend_from_slice(bytes);
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], SeenError> {
        let end = self
            .at
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(SeenError::Corrupt("a record ends early"))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }
    fn u8(&mut self) -> Result<u8, SeenError> {
        Ok(self.take(1)?[0])
    }
    fn bool(&mut self) -> Result<bool, SeenError> {
        Ok(self.u8()? != 0)
    }
    fn u32(&mut self) -> Result<u32, SeenError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn i32(&mut self) -> Result<i32, SeenError> {
        Ok(i32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn f32(&mut self) -> Result<f32, SeenError> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn bytes(&mut self) -> Result<&'a [u8], SeenError> {
        let count = self.u32()? as usize;
        self.take(count)
    }
    fn hash(&mut self) -> Result<PsoKey, SeenError> {
        Ok(self.take(32)?.try_into().expect("32 bytes"))
    }
}

fn write_bytecode(out: &mut Writer, code: &Bytecode) {
    out.u32(code.0.len() as u32);
    if !code.0.is_empty() {
        out.0.extend_from_slice(&Sha256::digest(&code.0));
    }
}

fn read_bytecode(input: &mut Reader<'_>, store: &dyn BytecodeStore) -> Result<Bytecode, SeenError> {
    let length = input.u32()? as usize;
    if length == 0 {
        return Ok(Bytecode::default());
    }
    let hash = input.hash()?;
    let bytes = store.get(&hash).ok_or(SeenError::MissingBytecode(hash))?;
    if bytes.len() != length || Sha256::digest(&bytes)[..] != hash {
        return Err(SeenError::Corrupt(
            "stored bytecode is not what was recorded",
        ));
    }
    Ok(Bytecode(bytes))
}

fn write_root(out: &mut Writer, root: &RootSignature) {
    out.bytes(&root.serialize());
}

fn read_root(input: &mut Reader<'_>) -> Result<Arc<RootSignature>, SeenError> {
    Ok(Arc::new(RootSignature::parse(input.bytes()?)?))
}

fn write_stencil(out: &mut Writer, face: &StencilFace) {
    for value in [face.fail, face.depth_fail, face.pass, face.func] {
        out.u32(value);
    }
}

fn read_stencil(input: &mut Reader<'_>) -> Result<StencilFace, SeenError> {
    Ok(StencilFace {
        fail: input.u32()?,
        depth_fail: input.u32()?,
        pass: input.u32()?,
        func: input.u32()?,
    })
}

fn encode(pso: &Pso) -> Vec<u8> {
    let mut out = Writer::default();
    match pso {
        Pso::Compute(desc) => {
            out.u8(COMPUTE);
            out.u32(pso.bytecode_len());
            out.bool(desc.root.is_some());
            if let Some(root) = &desc.root {
                write_root(&mut out, root);
            }
            write_bytecode(&mut out, &desc.cs);
        }
        Pso::Graphics(desc) => {
            out.u8(GRAPHICS);
            out.u32(pso.bytecode_len());
            write_root(&mut out, &desc.root);
            for code in [&desc.vs, &desc.ps, &desc.gs, &desc.ms] {
                write_bytecode(&mut out, code);
            }
            out.bool(desc.blend.alpha_to_coverage);
            out.bool(desc.blend.independent);
            for target in &desc.blend.targets {
                out.bool(target.blend_enable);
                out.bool(target.logic_enable);
                for value in [
                    target.src,
                    target.dst,
                    target.op,
                    target.src_alpha,
                    target.dst_alpha,
                    target.op_alpha,
                    target.logic_op,
                ] {
                    out.u32(value);
                }
                out.u8(target.write_mask);
            }
            out.u32(desc.sample_mask);
            let r = &desc.raster;
            out.u32(r.fill);
            out.u32(r.cull);
            out.bool(r.front_ccw);
            out.i32(r.depth_bias);
            out.f32(r.depth_bias_clamp);
            out.f32(r.slope_scaled_bias);
            out.bool(r.depth_clip);
            out.bool(r.multisample);
            out.u32(r.forced_sample_count);
            out.bool(r.conservative);
            let d = &desc.depth_stencil;
            out.bool(d.depth_enable);
            out.bool(d.depth_write);
            out.u32(d.depth_func);
            out.bool(d.stencil_enable);
            out.u8(d.stencil_read_mask);
            out.u8(d.stencil_write_mask);
            write_stencil(&mut out, &d.front);
            write_stencil(&mut out, &d.back);
            out.u32(desc.input_layout.len() as u32);
            for element in &desc.input_layout {
                out.bytes(element.semantic.as_bytes());
                for value in [
                    element.semantic_index,
                    element.format,
                    element.slot,
                    element.offset,
                    element.class,
                    element.step_rate,
                ] {
                    out.u32(value);
                }
            }
            out.u32(desc.strip_cut);
            out.u32(desc.topology);
            out.u32(desc.render_targets.len() as u32);
            for format in &desc.render_targets {
                out.u32(*format);
            }
            out.u32(desc.depth_format);
            out.u32(desc.sample_count);
        }
    }
    out.0
}

fn decode(body: &[u8], store: &dyn BytecodeStore) -> Result<Pso, SeenError> {
    let mut input = Reader { bytes: body, at: 0 };
    let kind = input.u8()?;
    input.u32()?;
    match kind {
        COMPUTE => {
            let root = if input.bool()? {
                Some(read_root(&mut input)?)
            } else {
                None
            };
            Ok(Pso::Compute(ComputePsoDesc {
                root,
                cs: read_bytecode(&mut input, store)?,
            }))
        }
        GRAPHICS => {
            let mut desc = GraphicsPsoDesc::new(read_root(&mut input)?);
            desc.vs = read_bytecode(&mut input, store)?;
            desc.ps = read_bytecode(&mut input, store)?;
            desc.gs = read_bytecode(&mut input, store)?;
            desc.ms = read_bytecode(&mut input, store)?;
            let mut blend = BlendState {
                alpha_to_coverage: input.bool()?,
                independent: input.bool()?,
                ..BlendState::default()
            };
            for target in &mut blend.targets {
                *target = RenderTargetBlend {
                    blend_enable: input.bool()?,
                    logic_enable: input.bool()?,
                    src: input.u32()?,
                    dst: input.u32()?,
                    op: input.u32()?,
                    src_alpha: input.u32()?,
                    dst_alpha: input.u32()?,
                    op_alpha: input.u32()?,
                    logic_op: input.u32()?,
                    write_mask: input.u8()?,
                };
            }
            desc.blend = blend;
            desc.sample_mask = input.u32()?;
            desc.raster = RasterizerState {
                fill: input.u32()?,
                cull: input.u32()?,
                front_ccw: input.bool()?,
                depth_bias: input.i32()?,
                depth_bias_clamp: input.f32()?,
                slope_scaled_bias: input.f32()?,
                depth_clip: input.bool()?,
                multisample: input.bool()?,
                forced_sample_count: input.u32()?,
                conservative: input.bool()?,
            };
            desc.depth_stencil = DepthStencilState {
                depth_enable: input.bool()?,
                depth_write: input.bool()?,
                depth_func: input.u32()?,
                stencil_enable: input.bool()?,
                stencil_read_mask: input.u8()?,
                stencil_write_mask: input.u8()?,
                front: read_stencil(&mut input)?,
                back: read_stencil(&mut input)?,
            };
            let elements = input.u32()? as usize;
            for _ in 0..elements {
                let semantic = String::from_utf8_lossy(input.bytes()?).into_owned();
                desc.input_layout.push(InputElement {
                    semantic,
                    semantic_index: input.u32()?,
                    format: input.u32()?,
                    slot: input.u32()?,
                    offset: input.u32()?,
                    class: input.u32()?,
                    step_rate: input.u32()?,
                });
            }
            desc.strip_cut = input.u32()?;
            desc.topology = input.u32()?;
            let targets = input.u32()? as usize;
            for _ in 0..targets {
                desc.render_targets.push(input.u32()?);
            }
            desc.depth_format = input.u32()?;
            desc.sample_count = input.u32()?;
            Ok(Pso::Graphics(Box::new(desc)))
        }
        _ => Err(SeenError::Corrupt("an unknown pipeline kind")),
    }
}

// the file

fn checksum(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for &b in bytes {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn frame(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 9);
    out.push(tag);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    let sum = checksum(&out);
    out.extend_from_slice(&sum.to_le_bytes());
    out
}

/// The framed records of a file's bytes, up to the first one that is torn or damaged.
fn frames(bytes: &[u8]) -> (Vec<(u8, &[u8])>, usize) {
    let mut found = Vec::new();
    let mut at = HEADER;
    while let Some(head) = bytes.get(at..at + 5) {
        let length = u32::from_le_bytes(head[1..5].try_into().expect("4 bytes")) as usize;
        let Some(end) = (at + 5).checked_add(length).and_then(|e| e.checked_add(4)) else {
            break;
        };
        let Some(record) = bytes.get(at..end) else {
            break;
        };
        let (body, sum) = record.split_at(record.len() - 4);
        if checksum(body) != u32::from_le_bytes(sum.try_into().expect("4 bytes")) {
            break;
        }
        found.push((head[0], &body[5..]));
        at = end;
    }
    (found, at)
}

/// The session that last created a pipeline, its place in that session, and its encoded description.
type Latest = (u64, usize, Arc<[u8]>);

/// One recorded pipeline: when it was last created and its description, kept encoded until it is wanted.
#[derive(Debug, Clone)]
pub struct SeenEntry {
    pub key: PsoKey,
    /// The session that last created it.
    pub session: u64,
    body: Arc<[u8]>,
}

impl SeenEntry {
    /// The bytes of the shaders the description holds, without loading it.
    pub fn bytecode_len(&self) -> u32 {
        self.body
            .get(1..5)
            .map_or(0, |b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
    }

    /// The description, with its bytecode read back from `store`.
    pub fn load(&self, store: &dyn BytecodeStore) -> Result<Pso, SeenError> {
        decode(&self.body, store)
    }
}

struct Appender {
    file: std::fs::File,
    recorded: HashSet<PsoKey>,
    stored: HashSet<PsoKey>,
}

pub struct SeenLog {
    path: PathBuf,
    session: u64,
    warm: Vec<SeenEntry>,
    bytecode: Arc<dyn BytecodeStore>,
    appender: Mutex<Appender>,
}

impl SeenLog {
    /// Opens the log at `path`, reading what earlier sessions recorded and starting a new session. Sessions older
    /// than the last `keep_sessions` are forgotten. A missing or unreadable file is an empty log.
    pub fn open(
        path: &Path,
        bytecode: Arc<dyn BytecodeStore>,
        keep_sessions: u64,
    ) -> Result<SeenLog, SeenError> {
        let io = |source| SeenError::Io {
            path: path.to_path_buf(),
            source,
        };
        let bytes = std::fs::read(path).unwrap_or_default();
        let readable = bytes.len() >= HEADER
            && bytes[..4] == MAGIC
            && u32::from_le_bytes(bytes[4..8].try_into().expect("4 bytes")) == VERSION;
        let (records, valid) = if readable {
            frames(&bytes)
        } else {
            (Vec::new(), 0)
        };

        let mut latest: HashMap<PsoKey, Latest> = HashMap::new();
        let (mut session, mut last, mut position, mut seen_records) = (0u64, 0u64, 0usize, 0usize);
        for (tag, payload) in records {
            match tag {
                SESSION if payload.len() == 8 => {
                    session = u64::from_le_bytes(payload.try_into().expect("8 bytes"));
                    last = last.max(session);
                    position = 0;
                }
                PIPELINE if payload.len() > 32 => {
                    seen_records += 1;
                    let key: PsoKey = payload[..32].try_into().expect("32 bytes");
                    latest.insert(key, (session, position, payload[32..].into()));
                    position += 1;
                }
                _ => {}
            }
        }
        let now = last + 1;
        let before = latest.len();
        latest.retain(|_, (seen, ..)| *seen + keep_sessions.max(1) >= now);
        let rewrite = !readable
            || valid < bytes.len()
            || latest.len() < before
            || seen_records > 2 * latest.len() + 64;

        let mut ordered: Vec<(PsoKey, Latest)> = latest.into_iter().collect();
        ordered.sort_by_key(|(_, (session, position, _))| (*session, *position));

        let mut file = if rewrite {
            let mut name = path.file_name().unwrap_or_default().to_os_string();
            name.push(".tmp");
            let temporary = path.with_file_name(name);
            if let Some(folder) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(folder).map_err(io)?;
            }
            let mut out = Vec::new();
            out.extend_from_slice(&MAGIC);
            out.extend_from_slice(&VERSION.to_le_bytes());
            let mut current = None;
            for (key, (session, _, body)) in &ordered {
                if current != Some(*session) {
                    out.extend(frame(SESSION, &session.to_le_bytes()));
                    current = Some(*session);
                }
                let mut payload = key.to_vec();
                payload.extend_from_slice(body);
                out.extend(frame(PIPELINE, &payload));
            }
            std::fs::write(&temporary, &out).map_err(io)?;
            std::fs::rename(&temporary, path).map_err(io)?;
            std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .map_err(io)?
        } else {
            std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .map_err(io)?
        };
        file.write_all(&frame(SESSION, &now.to_le_bytes()))
            .map_err(io)?;

        ordered.sort_by_key(|(_, (session, position, _))| (std::cmp::Reverse(*session), *position));
        let warm = ordered
            .into_iter()
            .map(|(key, (session, _, body))| SeenEntry { key, session, body })
            .collect();
        Ok(SeenLog {
            path: path.to_path_buf(),
            session: now,
            warm,
            bytecode,
            appender: Mutex::new(Appender {
                file,
                recorded: HashSet::new(),
                stored: HashSet::new(),
            }),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// This session's number: one more than the latest the file had.
    pub fn session(&self) -> u64 {
        self.session
    }

    /// What earlier sessions created, in the order to build it: the latest session's first, each in creation order.
    pub fn warm_list(&self) -> &[SeenEntry] {
        &self.warm
    }

    pub fn bytecode(&self) -> &Arc<dyn BytecodeStore> {
        &self.bytecode
    }

    /// Notes that this session created `pso`, once per session. True when it was written.
    pub fn record(&self, pso: &Pso) -> Result<bool, SeenError> {
        let key = pso.key();
        let mut appender = self.appender.lock().expect("seen log");
        if !appender.recorded.insert(key) {
            return Ok(false);
        }
        for code in pso.bytecodes().into_iter().filter(|c| !c.is_empty()) {
            let hash: PsoKey = Sha256::digest(&code.0).into();
            if appender.stored.insert(hash) {
                self.bytecode.put(&hash, &code.0);
            }
        }
        let mut payload = key.to_vec();
        payload.extend(encode(pso));
        appender
            .file
            .write_all(&frame(PIPELINE, &payload))
            .map_err(|source| SeenError::Io {
                path: self.path.clone(),
                source,
            })?;
        Ok(true)
    }

    /// Writes what the operating system is holding back.
    pub fn flush(&self) -> Result<(), SeenError> {
        self.appender
            .lock()
            .expect("seen log")
            .file
            .sync_data()
            .map_err(|source| SeenError::Io {
                path: self.path.clone(),
                source,
            })
    }
}
