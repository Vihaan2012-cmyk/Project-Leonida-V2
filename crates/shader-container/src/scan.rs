use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::ops::ControlFlow;

use crate::{
    CarveStats, CarvedShader, CodeRange, Fssm, ShaderModel, Stage, WaveSize, carve, parts,
    unpack_all,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShaderInfo {
    pub name: String,
    pub offset: usize,
    pub stage: Option<Stage>,
    pub shader_model: Option<ShaderModel>,
    pub container_size: usize,
    pub parts: Vec<String>,
    pub payloads: usize,
    pub fssm_size: Option<usize>,
    pub fssm_version: Option<u16>,
    pub wave: Option<WaveSize>,
    pub code_range: Option<CodeRange>,
    pub problem: Option<String>,
}

impl ShaderInfo {
    pub fn stage_name(&self) -> String {
        self.stage
            .map_or_else(|| "none".to_string(), Stage::short_name)
    }

    pub fn describe(shader: &CarvedShader) -> ShaderInfo {
        let mut info = ShaderInfo {
            name: shader.name.clone(),
            offset: shader.offset,
            stage: shader.stage,
            shader_model: shader.shader_model,
            container_size: shader.dxbc.len(),
            parts: Vec::new(),
            payloads: 0,
            fssm_size: None,
            fssm_version: None,
            wave: None,
            code_range: None,
            problem: None,
        };
        if let Ok(container_parts) = parts(&shader.dxbc) {
            info.parts = container_parts
                .iter()
                .map(|part| String::from_utf8_lossy(&part.fourcc).into_owned())
                .collect();
        }
        let blobs = match unpack_all(&shader.dxbc) {
            Ok(blobs) => blobs,
            Err(error) => {
                info.problem = Some(error.to_string());
                return info;
            }
        };
        info.payloads = blobs.len();
        let Some(last) = blobs.last() else {
            info.problem = Some("no XBCH payload".to_string());
            return info;
        };
        info.fssm_size = Some(last.len());
        match Fssm::parse(last) {
            Ok(fssm) => {
                info.fssm_version = Some(fssm.version());
                info.wave = Some(fssm.wave_size());
                info.code_range = Some(fssm.code_range());
            }
            Err(error) => info.problem = Some(error.to_string()),
        }
        info
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub stats: CarveStats,
    pub shaders: Vec<ShaderInfo>,
    pub cancelled: bool,
}

pub fn scan(archive: &[u8], mut progress: impl FnMut(usize) -> ControlFlow<()>) -> ScanResult {
    let mut result = ScanResult::default();
    result.stats = carve(archive, |shader| {
        result.shaders.push(ShaderInfo::describe(&shader));
        let flow = progress(shader.offset);
        if flow.is_break() {
            result.cancelled = true;
        }
        flow
    });
    result
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanSummary {
    pub carved: usize,
    pub rejected: usize,
    pub stages: BTreeMap<String, usize>,
    pub parts: BTreeMap<String, usize>,
    pub payloads: usize,
    pub multi_payload: usize,
    pub wave_by_stage: BTreeMap<String, usize>,
    pub wave: BTreeMap<&'static str, usize>,
    pub ranges: BTreeMap<&'static str, usize>,
    pub problems: BTreeMap<String, usize>,
}

pub fn wave_name(wave: WaveSize) -> &'static str {
    match wave {
        WaveSize::Wave32 => "wave32",
        WaveSize::Wave64 => "wave64",
    }
}

pub fn range_name(range: CodeRange) -> &'static str {
    match range {
        CodeRange::Header { .. } => "header",
        CodeRange::Scan { .. } => "scan",
        CodeRange::NotFound => "not found",
    }
}

impl ScanSummary {
    pub fn of(result: &ScanResult) -> ScanSummary {
        let mut summary = ScanSummary {
            carved: result.stats.carved,
            rejected: result.stats.rejected,
            ..ScanSummary::default()
        };
        for info in &result.shaders {
            let stage = info.stage_name();
            *summary.stages.entry(stage.clone()).or_default() += 1;
            for part in &info.parts {
                *summary.parts.entry(part.clone()).or_default() += 1;
            }
            summary.payloads += info.payloads;
            if info.payloads > 1 {
                summary.multi_payload += 1;
            }
            if let Some(wave) = info.wave {
                *summary.wave.entry(wave_name(wave)).or_default() += 1;
                *summary
                    .wave_by_stage
                    .entry(format!("{stage} {}", wave_name(wave)))
                    .or_default() += 1;
            }
            if let Some(range) = info.code_range {
                *summary.ranges.entry(range_name(range)).or_default() += 1;
            }
            if let Some(problem) = &info.problem {
                *summary.problems.entry(problem.clone()).or_default() += 1;
            }
        }
        summary
    }

    pub fn report(&self) -> String {
        let mut out = String::new();
        let w = &mut out;
        writeln!(
            w,
            "shaders.rpf: {} containers carved, {} candidates rejected",
            self.carved, self.rejected
        )
        .ok();
        writeln!(w, "stages: {}", counts(&self.stages)).ok();
        writeln!(w, "container parts: {}", counts(&self.parts)).ok();
        writeln!(
            w,
            "FSSM payloads: {} ({} container{} more than one; the last is used below)",
            self.payloads,
            self.multi_payload,
            if self.multi_payload == 1 {
                " has"
            } else {
                "s have"
            }
        )
        .ok();
        writeln!(w, "wave size by stage: {}", counts(&self.wave_by_stage)).ok();
        writeln!(w, "wave size: {}", counts(&self.wave)).ok();
        writeln!(w, "code range: {}", counts(&self.ranges)).ok();
        if self.problems.is_empty() {
            writeln!(w, "problems: none").ok();
        } else {
            writeln!(w, "problems:").ok();
            for (problem, count) in &self.problems {
                writeln!(w, "  {count} x {problem}").ok();
            }
        }
        out
    }
}

pub fn counts<K: std::fmt::Display>(map: &BTreeMap<K, usize>) -> String {
    let parts: Vec<String> = map
        .iter()
        .map(|(key, count)| format!("{key} {count}"))
        .collect();
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{container, fssm, rpf_entry};

    const NOP: u32 = 0xBF80_0000;

    fn archive() -> Vec<u8> {
        let mut archive = vec![0u8; 8];
        archive.extend(rpf_entry(
            "ps_one",
            3,
            1,
            &container(0, &[fssm(false, &[NOP])]),
        ));
        archive.push(0);
        let two = container(5, &[fssm(true, &[NOP]), fssm(true, &[NOP, NOP])]);
        archive.extend(rpf_entry("cs_two", 3, 2, &two));
        archive
    }

    #[test]
    fn describes_every_container() {
        let result = scan(&archive(), |_| ControlFlow::Continue(()));
        assert!(!result.cancelled);
        assert_eq!(result.shaders.len(), 2);
        let ps = &result.shaders[0];
        assert_eq!(ps.name, "ps_one");
        assert_eq!(ps.stage_name(), "ps");
        assert_eq!(ps.parts, vec!["DXIL".to_string(), "PRIV".to_string()]);
        assert_eq!(ps.payloads, 1);
        assert_eq!(ps.wave, Some(WaveSize::Wave64));
        assert_eq!(
            ps.code_range,
            Some(CodeRange::Header {
                start: 0x128,
                end: 0x130
            })
        );
        assert_eq!(ps.fssm_version, Some(1));
        assert_eq!(ps.problem, None);
        assert_eq!(result.shaders[1].payloads, 2);
        assert_eq!(result.shaders[1].wave, Some(WaveSize::Wave32));
    }

    #[test]
    fn summary_report_matches_the_cli_format() {
        let result = scan(&archive(), |_| ControlFlow::Continue(()));
        let report = ScanSummary::of(&result).report();
        assert!(report.contains("shaders.rpf: 2 containers carved, 0 candidates rejected"));
        assert!(report.contains("stages: cs 1, ps 1"));
        assert!(report.contains("container parts: DXIL 2, PRIV 2"));
        assert!(
            report.contains(
                "FSSM payloads: 3 (1 container has more than one; the last is used below)"
            )
        );
        assert!(report.contains("wave size by stage: cs wave32 1, ps wave64 1"));
        assert!(report.contains("wave size: wave32 1, wave64 1"));
        assert!(report.contains("code range: header 2"));
        assert!(report.contains("problems: none"));
    }

    #[test]
    fn progress_can_cancel() {
        let mut calls = 0;
        let result = scan(&archive(), |_| {
            calls += 1;
            ControlFlow::Break(())
        });
        assert!(result.cancelled);
        assert_eq!(calls, 1);
        assert_eq!(result.shaders.len(), 1);
    }

    #[test]
    fn unreadable_payloads_are_problems() {
        let mut broken = container(0, &[fssm(false, &[NOP])]);
        // Corrupt the XBCH magic inside the container.
        let at = broken.windows(4).position(|w| w == b"XBCH").unwrap();
        broken[at] = b'Y';
        let mut archive = vec![0u8];
        archive.extend(rpf_entry("ps_bad", 3, 1, &broken));
        let result = scan(&archive, |_| ControlFlow::Continue(()));
        assert!(result.shaders[0].problem.is_some());
        assert_eq!(ScanSummary::of(&result).problems.len(), 1);
    }
}
