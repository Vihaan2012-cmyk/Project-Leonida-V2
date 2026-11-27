//! Where each root parameter lands in the user-data words, so a changed argument is written into the push block
//! in place instead of packing every parameter again.

use crate::root::{Place, RootArgument, RootError, StageLayout};

/// The user-data placements of every root parameter for one pipeline's stages.
#[derive(Debug, Default)]
pub struct Plan {
    /// By parameter: the `(first SGPR, dwords)` of each stage that takes it inline.
    places: Vec<Vec<(u32, u32)>>,
}

impl Plan {
    /// The plan for `layouts`, reusing this one's storage. A signature that spills has no plan: its array is
    /// packed whole and stored where the shaders can read it, so every change repacks.
    pub fn build(&mut self, layouts: &[&StageLayout]) -> bool {
        for list in &mut self.places {
            list.clear();
        }
        if layouts.iter().any(|l| l.spills()) {
            return false;
        }
        for layout in layouts {
            if self.places.len() < layout.places.len() {
                self.places.resize_with(layout.places.len(), Vec::new);
            }
            for (param, place) in layout.places.iter().enumerate() {
                if let Place::Inline { sgpr, dwords } = *place {
                    self.places[param].push((sgpr, dwords));
                }
            }
        }
        true
    }

    /// Writes the dwords of `argument` for `param` into `sgprs` where every stage reads them.
    pub fn patch(
        &self,
        sgprs: &mut [u32],
        param: u32,
        argument: Option<&RootArgument>,
    ) -> Result<(), RootError> {
        let Some(places) = self.places.get(param as usize) else {
            return Ok(());
        };
        for &(sgpr, dwords) in places {
            for k in 0..dwords {
                let value = dword(argument, param, k)?;
                if let Some(slot) = sgprs.get_mut((sgpr + k) as usize) {
                    *slot = value;
                }
            }
        }
        Ok(())
    }
}

fn dword(argument: Option<&RootArgument>, param: u32, k: u32) -> Result<u32, RootError> {
    Ok(match argument {
        Some(RootArgument::Constants(list)) => list.get(k as usize).copied().unwrap_or(0),
        Some(RootArgument::View(va)) => (va >> (32 * k)) as u32,
        Some(RootArgument::Table(handle)) => {
            u32::try_from(*handle).map_err(|_| RootError::TableAboveFourGiB {
                param,
                handle: *handle,
            })?
        }
        _ => 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::{
        DescriptorRange, HeapBases, ParameterKind, RangeType, RootArguments, RootParameter,
        RootSignature, RootVersion, UserStage, Visibility, pack,
    };

    fn signature(parameters: Vec<RootParameter>) -> RootSignature {
        RootSignature {
            version: RootVersion::V1_1,
            parameters,
            static_samplers: Vec::new(),
            flags: 0,
        }
    }

    fn parameters(constants: u32) -> Vec<RootParameter> {
        vec![
            RootParameter {
                kind: ParameterKind::Constants {
                    register: 0,
                    space: 0,
                    values: constants,
                },
                visibility: Visibility::All,
            },
            RootParameter {
                kind: ParameterKind::Table(vec![DescriptorRange {
                    kind: RangeType::Srv,
                    count: 4,
                    base_register: 0,
                    space: 0,
                    flags: 0,
                    offset: 0xFFFF_FFFF,
                }]),
                visibility: Visibility::Pixel,
            },
            RootParameter {
                kind: ParameterKind::Cbv {
                    register: 1,
                    space: 0,
                    flags: 0,
                },
                visibility: Visibility::Vertex,
            },
        ]
    }

    #[test]
    fn patching_an_argument_gives_the_words_a_full_pack_gives() {
        let sig = signature(parameters(4));
        let vertex = sig.layout(UserStage::Vertex).unwrap();
        let pixel = sig.layout(UserStage::Pixel).unwrap();
        let layouts = [&vertex, &pixel];
        let mut plan = Plan::default();
        assert!(plan.build(&layouts));
        let mut args = RootArguments::new(&sig);
        let mut words = pack(&layouts, &args, HeapBases::default()).unwrap().sgprs;
        args.set_constants(0, 1, &[7, 8]).unwrap();
        plan.patch(&mut words, 0, args.get(0)).unwrap();
        args.set_table(1, 0x1234).unwrap();
        plan.patch(&mut words, 1, args.get(1)).unwrap();
        args.set_view(2, 0x5_0000_0040).unwrap();
        plan.patch(&mut words, 2, args.get(2)).unwrap();
        let packed = pack(&layouts, &args, HeapBases::default()).unwrap();
        assert_eq!(words, packed.sgprs);
        assert_ne!(words, [0; 32]);
    }

    #[test]
    fn a_table_above_four_gib_is_refused_as_a_pack_refuses_it() {
        let sig = signature(parameters(2));
        let pixel = sig.layout(UserStage::Pixel).unwrap();
        let mut plan = Plan::default();
        assert!(plan.build(&[&pixel]));
        let mut args = RootArguments::new(&sig);
        args.set_table(1, 1 << 32).unwrap();
        let mut words = [0; 32];
        assert!(matches!(
            plan.patch(&mut words, 1, args.get(1)),
            Err(RootError::TableAboveFourGiB { .. })
        ));
    }

    #[test]
    fn a_signature_that_spills_has_no_plan() {
        let sig = signature(parameters(27));
        let layout = sig.layout(UserStage::Compute).unwrap();
        assert!(layout.spills());
        assert!(!Plan::default().build(&[&layout]));
    }

    #[test]
    fn rebuilding_for_other_layouts_forgets_the_old_places() {
        let sig = signature(parameters(4));
        let vertex = sig.layout(UserStage::Vertex).unwrap();
        let mesh = sig.layout(UserStage::Mesh).unwrap();
        let mut plan = Plan::default();
        plan.build(&[&vertex]);
        let mut words = [0; 32];
        plan.build(&[&mesh]);
        let mut args = RootArguments::new(&sig);
        args.set_constants(0, 0, &[9]).unwrap();
        plan.patch(&mut words, 0, args.get(0)).unwrap();
        assert_eq!(words[16], 9);
        assert_eq!(words[8], 0);
    }
}
