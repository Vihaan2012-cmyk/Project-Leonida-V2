//! Replays the seen-pipeline log of a real session through the stage translation, gated on `LEONIDA_SEEN_LOG`
//! (the log file; its shaders sit in `<log>.shaders`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use shader_container::{Stage, parts, shader_model};
use shader_translate::FloatMode;

use super::{DirectTranslation, Link, ShaderSource};
use crate::compile_queue::{DirBytecode, Pso, read_entries};

fn declared(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "none".into();
    }
    if bytes.starts_with(b"FSSM") {
        return "bare-fssm".into();
    }
    match parts(bytes) {
        Err(e) => format!("unparsed:{e}"),
        Ok(list) => match shader_model(&list) {
            Ok(Some((stage, _))) => stage.short_name().to_string(),
            other => format!("model:{other:?}"),
        },
    }
}

#[test]
fn the_seen_log_translates() {
    let Some(log) = std::env::var_os("LEONIDA_SEEN_LOG").map(PathBuf::from) else {
        return;
    };
    let mut shaders = log.clone().into_os_string();
    shaders.push(".shaders");
    let store = DirBytecode::new(PathBuf::from(shaders));
    let source = DirectTranslation::new(FloatMode::default());
    let mut layouts: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures: BTreeMap<String, usize> = BTreeMap::new();
    let (mut total, mut ok) = (0, 0);
    for entry in read_entries(&log) {
        let Ok(Pso::Graphics(desc)) = entry.load(&store) else {
            continue;
        };
        total += 1;
        let key = format!(
            "vs={} ps={} gs={} ms={}",
            declared(&desc.vs.0),
            declared(&desc.ps.0),
            declared(&desc.gs.0),
            declared(&desc.ms.0)
        );
        *layouts.entry(key).or_default() += 1;
        let result = (|| {
            let first_half = !desc.gs.is_empty();
            let vs = source.translate(
                &desc.vs.0,
                Stage::Vertex,
                &Link {
                    previous: None,
                    first_half,
                },
            )?;
            let mut previous: Arc<_> = vs;
            if first_half {
                previous = source.translate(&desc.gs.0, Stage::Geometry, &Link::default())?;
            }
            if !desc.ps.is_empty() {
                source.translate(
                    &desc.ps.0,
                    Stage::Pixel,
                    &Link {
                        previous: Some(previous),
                        first_half: false,
                    },
                )?;
            }
            Ok::<_, crate::pipeline::PipelineError>(())
        })();
        match result {
            Ok(()) => ok += 1,
            Err(e) => *failures.entry(e.to_string()).or_default() += 1,
        }
    }
    eprintln!("pipelines {total}, translated {ok}");
    for (k, v) in &layouts {
        eprintln!("  {v:5} {k}");
    }
    for (k, v) in &failures {
        eprintln!("  fail {v:5} {k}");
    }
}
