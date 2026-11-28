//! The code patches the settings turn on, applied to the loaded image before any game thread exists.

use anyhow::{Context, Result, anyhow};
use enhancements::framerate;
use enhancements::patch::{CodeMemory, Finder, Mapped, Patch, apply_all};
use enhancements::widescreen::{CodeImage, PatchGroup, PatchOp, rel32};
use pe_image::Image;

use super::plan::RuntimePlan;

/// What happened to the patches: the names applied, or why none were.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub applied: Vec<String>,
    pub warning: Option<String>,
}

/// The loaded image as the widescreen checks read it.
struct Memory<'a>(&'a dyn CodeMemory);

impl CodeImage for Memory<'_> {
    fn read(&self, rva: u32, len: usize) -> Option<Vec<u8>> {
        self.0.read(rva, len).ok()
    }
}

/// A patch group as byte patches. `slot` makes a read-only f32 in reach of the image and gives its address.
pub fn group_patches(
    group: &PatchGroup,
    memory: &dyn CodeMemory,
    image_base: usize,
    slot: &mut dyn FnMut(f32) -> Result<usize, String>,
) -> Result<Vec<Patch>> {
    group.validate(&Memory(memory))?;
    let mut patches = Vec::new();
    for op in &group.ops {
        match op {
            PatchOp::Bytes { rva, replace, .. } => {
                let original = memory.read(*rva, replace.len())?;
                patches.push(Patch {
                    name: format!("{} at {rva:#x}", group.id),
                    rva: *rva,
                    original,
                    replacement: replace.clone(),
                });
            }
            PatchOp::RipConstant {
                rva,
                len,
                expect_target,
                value,
            } => {
                let next = *rva + u32::from(*len);
                let address = slot(*value).map_err(|e| anyhow!("{}: {e}", group.id))?;
                let from = image_base as i64 + i64::from(next);
                let displacement = i32::try_from(address as i64 - from).map_err(|_| {
                    anyhow!(
                        "{}: the constant at {address:#x} is out of reach of {rva:#x}",
                        group.id
                    )
                })?;
                patches.push(Patch {
                    name: format!("{} at {rva:#x}", group.id),
                    rva: next - 4,
                    original: rel32(next, *expect_target).to_vec(),
                    replacement: displacement.to_le_bytes().to_vec(),
                });
            }
        }
    }
    Ok(patches)
}

/// Every patch the plan asks for. Nothing is written.
pub fn select(
    plan: &RuntimePlan,
    bytes: &[u8],
    memory: &dyn CodeMemory,
    image_base: usize,
    slot: &mut dyn FnMut(f32) -> Result<usize, String>,
) -> Result<Vec<Patch>> {
    let mut patches = Vec::new();
    if plan.custom_limit {
        let image = Image::parse(bytes).context("reading the executable's sections")?;
        let finder = Finder::new(&image);
        let without: &[&str] = if plan.graphics.fix_frame_timers {
            &[]
        } else {
            &[framerate::FRAME_TIMERS.id]
        };
        patches.extend(framerate::select_without(
            &finder,
            plan.custom_limit,
            &[],
            without,
        )?);
    }
    // The widescreen groups are applied here, with the framerate patches, as one set.
    for group in plan.widescreen.groups()? {
        patches.extend(group_patches(&group, memory, image_base, slot)?);
    }
    Ok(patches)
}

/// Writes every patch or none.
pub fn write_all(memory: &mut dyn CodeMemory, patches: &[Patch]) -> Result<Vec<String>> {
    let applied = apply_all(memory, patches)?;
    Ok(applied
        .iter()
        .map(|done| done.patch().name.clone())
        .collect())
}

/// Applies the plan's patches to the loaded image. A failure applies none, leaves the game as it is and says why.
#[allow(unsafe_code)]
pub fn apply(plan: &RuntimePlan, bytes: &[u8], module: &pe_loader::Module) -> Outcome {
    let (base, size) = (module.base(), module.size());
    // SAFETY: `module` is the image just loaded and stays mapped for the whole run.
    let mut memory = unsafe { Mapped::new(base, size) };
    let mut slot = |value: f32| shims::constants::read_only_f32_near(base, size, value);
    let result = select(plan, bytes, &memory, base, &mut slot)
        .and_then(|patches| write_all(&mut memory, &patches));
    match result {
        Ok(applied) => Outcome {
            applied,
            warning: None,
        },
        Err(error) => Outcome {
            applied: Vec::new(),
            warning: Some(format!("{error:#}")),
        },
    }
}

#[cfg(test)]
mod tests {
    use enhancements::patch::Buffer;
    use enhancements::widescreen::{Confidence, Expect, fnv1a64};

    use super::*;

    const BASE: usize = 0x1_4000_0000;

    fn memory() -> Buffer {
        let mut bytes = vec![0u8; 0x100];
        bytes[0x10..0x14].copy_from_slice(&[0x90, 0x90, 0x75, 0x05]);
        let next = 0x28u32;
        bytes[0x24..0x28].copy_from_slice(&rel32(next, 0x80));
        Buffer(bytes)
    }

    fn group(ops: Vec<PatchOp>) -> PatchGroup {
        PatchGroup {
            id: "test_group",
            summary: "a test",
            confidence: Confidence::High,
            evidence: &[],
            ops,
        }
    }

    fn bytes_op() -> PatchOp {
        PatchOp::Bytes {
            rva: 0x12,
            expect: Expect::new(2, fnv1a64(&[0x75, 0x05])),
            replace: vec![0xEB],
        }
    }

    fn rip_op() -> PatchOp {
        PatchOp::RipConstant {
            rva: 0x20,
            len: 8,
            expect_target: 0x80,
            value: 1.5,
        }
    }

    #[test]
    fn a_group_becomes_byte_patches_the_loaded_image_accepts() {
        let mut memory = memory();
        let mut asked = Vec::new();
        let mut slot = |value: f32| {
            asked.push(value);
            Ok(BASE + 0x2000)
        };
        let patches =
            group_patches(&group(vec![bytes_op(), rip_op()]), &memory, BASE, &mut slot).unwrap();
        assert_eq!(asked, [1.5]);
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0].rva, 0x12);
        assert_eq!(patches[0].original, [0x75]);
        assert_eq!(patches[0].replacement, [0xEB]);
        assert_eq!(patches[1].rva, 0x24);
        assert_eq!(patches[1].replacement, (0x2000i32 - 0x28).to_le_bytes());
        let names = write_all(&mut memory, &patches).unwrap();
        assert_eq!(names, ["test_group at 0x12", "test_group at 0x20"]);
        assert_eq!(memory.0[0x12], 0xEB);
        assert_eq!(&memory.0[0x24..0x28], &(0x2000i32 - 0x28).to_le_bytes());
    }

    #[test]
    fn a_group_whose_bytes_are_not_in_the_image_is_refused_whole() {
        let memory = memory();
        let wrong = PatchOp::Bytes {
            rva: 0x10,
            expect: Expect::new(2, fnv1a64(&[0x74, 0x05])),
            replace: vec![0xEB],
        };
        let mut slot = |_| Ok(BASE + 0x2000);
        let error = group_patches(&group(vec![bytes_op(), wrong]), &memory, BASE, &mut slot)
            .unwrap_err()
            .to_string();
        assert!(error.contains("test_group"), "{error}");
        assert!(error.contains("0x10"), "{error}");
    }

    #[test]
    fn a_constant_out_of_reach_or_not_made_is_an_error() {
        let memory = memory();
        let mut far = |_| Ok(BASE + 0x1_0000_0000);
        let error = group_patches(&group(vec![rip_op()]), &memory, BASE, &mut far)
            .unwrap_err()
            .to_string();
        assert!(error.contains("out of reach"), "{error}");
        let mut none = |_| Err("no free page".to_string());
        let error = group_patches(&group(vec![rip_op()]), &memory, BASE, &mut none)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no free page"), "{error}");
    }

    #[test]
    fn a_patch_that_fails_to_apply_leaves_the_earlier_ones_undone() {
        let mut memory = memory();
        let good = Patch {
            name: "good".into(),
            rva: 0x12,
            original: vec![0x75],
            replacement: vec![0xEB],
        };
        let bad = Patch {
            name: "bad".into(),
            rva: 0x30,
            original: vec![0xFF],
            replacement: vec![0x00],
        };
        assert!(write_all(&mut memory, &[good, bad]).is_err());
        assert_eq!(memory.0[0x12], 0x75);
    }

    #[test]
    fn with_the_console_limit_and_no_widescreen_nothing_is_selected() {
        use crate::cli::RunArgs;
        let args = RunArgs {
            game: None,
            file: None,
            dry_run: false,
            start_the_game: true,
            all: false,
            trace: false,
            output: None,
            config: "E:\\Leo\\Config".into(),
            frame_limit: None,
        };
        let settings = super::super::plan::Settings {
            graphics: <config::Graphics as config::Section>::parse("frame_rate_mode = \"console\"")
                .unwrap(),
            ..super::super::plan::Settings::default()
        };
        let plan = RuntimePlan::from(&args, &settings, std::path::Path::new("E:\\Game")).unwrap();
        let mut slot = |_| Err("unused".to_string());
        let patches = select(&plan, &[], &memory(), BASE, &mut slot).unwrap();
        assert!(patches.is_empty());
    }
}
