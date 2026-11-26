use std::ops::ControlFlow;

use crate::dxbc::{ShaderModel, Stage, parts, shader_model, u32_at};
use crate::xdat::inflate;

const MIN_NAME: usize = 3;
const MAX_NAME: usize = 200;
/// kind u8, stored u32, id u32, stored u32, size u32
const HEADER: usize = 17;

/// One shader container found in the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarvedShader {
    pub name: String,
    /// Where the record's name starts in the archive.
    pub offset: usize,
    pub kind: u8,
    pub stage: Option<Stage>,
    pub shader_model: Option<ShaderModel>,
    pub dxbc: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CarveStats {
    pub carved: usize,
    /// Candidates whose header looked right but whose data did not inflate to a valid container.
    pub rejected: usize,
}

/// Find every shader record in `archive` (the bytes of shaders.rpf) without reading its encrypted table of
/// contents, and pass each one to `each` in archive order. `each` returning `ControlFlow::Break` stops the scan.
pub fn carve(archive: &[u8], mut each: impl FnMut(CarvedShader) -> ControlFlow<()>) -> CarveStats {
    let mut stats = CarveStats::default();
    let mut floor = 0; // names never start before the end of the last carved record
    let mut pos = 0;
    while let Some(found) = memchr::memchr(0, &archive[pos..]) {
        let nul = pos + found;
        pos = nul + 1;
        let Some(name_start) = name_start(archive, floor, nul) else {
            continue;
        };
        let Some(header) = archive.get(nul + 1..nul + 1 + HEADER) else {
            break;
        };
        let kind = header[0];
        let stored = u32_at(header, 1) as usize;
        let size = u32_at(header, 13) as usize;
        let plausible = stored == u32_at(header, 9) as usize
            && (17..64 << 20).contains(&stored)
            && (33..256 << 20).contains(&size);
        if !plausible {
            continue;
        }
        let data_start = nul + 1 + HEADER;
        let data_end = archive.len().min(data_start + stored);
        match decode(&archive[data_start..data_end], size) {
            Some((dxbc, stage, shader_model)) => {
                let name = String::from_utf8_lossy(&archive[name_start..nul]).into_owned();
                stats.carved += 1;
                pos = data_end;
                floor = data_end;
                let flow = each(CarvedShader {
                    name,
                    offset: name_start,
                    kind,
                    stage,
                    shader_model,
                    dxbc,
                });
                if flow.is_break() {
                    break;
                }
            }
            None => stats.rejected += 1,
        }
    }
    stats
}

/// Start of the name that ends at `nul`: the printable run before it, at most MAX_NAME bytes and never before
/// `floor`; None when shorter than MIN_NAME.
fn name_start(archive: &[u8], floor: usize, nul: usize) -> Option<usize> {
    let lowest = floor.max(nul.saturating_sub(MAX_NAME));
    let mut start = nul;
    while start > lowest && (0x21..=0x7E).contains(&archive[start - 1]) {
        start -= 1;
    }
    (nul - start >= MIN_NAME).then_some(start)
}

/// Inflate a record's raw deflate data to a DXBC container of exactly `size` bytes and read its stage.
fn decode(data: &[u8], size: usize) -> Option<(Vec<u8>, Option<Stage>, Option<ShaderModel>)> {
    let dxbc = inflate(data, size, false, false).ok()?;
    if !dxbc.starts_with(b"DXBC") {
        return None;
    }
    let model = shader_model(&parts(&dxbc).ok()?).ok()?;
    let (stage, shader_model) = match model {
        Some((stage, shader_model)) => (Some(stage), Some(shader_model)),
        None => (None, None),
    };
    Some((dxbc, stage, shader_model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{container, fssm, rpf_entry};

    const NOP: u32 = 0xBF80_0000;

    fn collect(archive: &[u8]) -> (Vec<CarvedShader>, CarveStats) {
        let mut found = Vec::new();
        let stats = carve(archive, |shader| {
            found.push(shader);
            ControlFlow::Continue(())
        });
        (found, stats)
    }

    #[test]
    fn break_stops_after_the_current_record() {
        let ps = container(0, &[fssm(false, &[NOP])]);
        let mut archive = vec![0u8];
        archive.extend(rpf_entry("ps_one", 3, 1, &ps));
        archive.push(0);
        archive.extend(rpf_entry("ps_two", 3, 2, &ps));
        let mut names = Vec::new();
        let stats = carve(&archive, |shader| {
            names.push(shader.name);
            ControlFlow::Break(())
        });
        assert_eq!(names, vec!["ps_one".to_string()]);
        assert_eq!(stats.carved, 1);
    }

    #[test]
    fn carves_records_between_junk() {
        let ps = container(0, &[fssm(false, &[NOP])]);
        let vs = container(1, &[fssm(true, &[NOP])]);
        let mut archive = b"\x01\x02junk\x00".to_vec();
        let first = archive.len();
        archive.extend(rpf_entry("ps_alpha", 3, 7, &ps));
        archive.extend_from_slice(b"\x00\x00more junk\x00");
        archive.extend(rpf_entry("vs_beta", 3, 8, &vs));
        let (found, stats) = collect(&archive);
        assert_eq!(
            stats,
            CarveStats {
                carved: 2,
                rejected: 0
            }
        );
        assert_eq!(found[0].name, "ps_alpha");
        assert_eq!(found[0].offset, first);
        assert_eq!(found[0].kind, 3);
        assert_eq!(found[0].stage, Some(Stage::Pixel));
        assert_eq!(
            found[0].shader_model,
            Some(ShaderModel { major: 6, minor: 6 })
        );
        assert_eq!(found[0].dxbc, ps);
        assert_eq!(found[1].name, "vs_beta");
        assert_eq!(found[1].stage, Some(Stage::Vertex));
        assert_eq!(found[1].dxbc, vs);
    }

    #[test]
    fn corrupt_deflate_is_rejected() {
        let ps = container(0, &[fssm(false, &[NOP])]);
        let mut entry = rpf_entry("ps_broken", 3, 1, &ps);
        let data_start = "ps_broken".len() + 1 + 17;
        for byte in &mut entry[data_start..] {
            *byte = 0xFF; // BTYPE 11 is reserved: inflate fails at once
        }
        let mut archive = vec![0u8];
        archive.extend(entry);
        let (found, stats) = collect(&archive);
        assert!(found.is_empty());
        assert_eq!(
            stats,
            CarveStats {
                carved: 0,
                rejected: 1
            }
        );
    }

    #[test]
    fn names_are_at_most_200_bytes() {
        let ps = container(0, &[fssm(false, &[NOP])]);
        let long = "a".repeat(250);
        let mut archive = vec![0u8];
        archive.extend(rpf_entry(&long, 3, 1, &ps));
        let (found, _) = collect(&archive);
        assert_eq!(found[0].name.len(), 200);
        assert_eq!(found[0].offset, 1 + 50);
    }

    #[test]
    fn short_names_and_empty_input_find_nothing() {
        let ps = container(0, &[fssm(false, &[NOP])]);
        let mut archive = vec![0u8];
        archive.extend(rpf_entry("ab", 3, 1, &ps));
        assert_eq!(
            collect(&archive).1,
            CarveStats {
                carved: 0,
                rejected: 0
            }
        );
        assert_eq!(collect(&[]).1, CarveStats::default());
    }
}
