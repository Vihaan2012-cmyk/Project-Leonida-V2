//! Which output of the stage before a pixel shader feeds each of its attributes.

use crate::fssm::{Interpolant, ParameterExport};
use crate::signature::SignatureElement;

/// Where each pixel-shader attribute comes from: entry N is the output location of the stage before that
/// attribute N reads, or `None` when that stage writes no output with the attribute's semantic. Elements are
/// matched by semantic name and index; the earlier stage's location for an output register is its parameter
/// number from `exports`.
pub fn pixel_attribute_sources(
    outputs: &[SignatureElement],
    exports: &[ParameterExport],
    inputs: &[SignatureElement],
    interpolants: &[Interpolant],
) -> Vec<Option<u8>> {
    interpolants
        .iter()
        .map(|interpolant| {
            inputs
                .iter()
                .filter(|input| input.register == u32::from(interpolant.register))
                .find_map(|input| {
                    let output = outputs.iter().find(|output| {
                        output.semantic.eq_ignore_ascii_case(&input.semantic)
                            && output.semantic_index == input.semantic_index
                    })?;
                    exports
                        .iter()
                        .find(|export| u32::from(export.register) == output.register)
                        .map(|export| export.parameter)
                })
        })
        .collect()
}

/// The exports of a translated geometry shader, whose outputs keep their register numbers as locations.
pub fn register_exports(outputs: &[SignatureElement]) -> Vec<ParameterExport> {
    let mut exports: Vec<ParameterExport> = outputs
        .iter()
        .filter(|output| output.system_value != 1)
        .map(|output| ParameterExport {
            kind: 6,
            register: output.register as u8,
            parameter: output.register as u8,
        })
        .collect();
    exports.dedup_by_key(|export| export.register);
    exports
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(semantic: &str, index: u32, register: u32) -> SignatureElement {
        SignatureElement {
            stream: 0,
            semantic: semantic.into(),
            semantic_index: index,
            system_value: u32::from(semantic == "SV_Position"),
            component_type: 3,
            register,
            mask: 0xF,
            rw_mask: 0xF,
            min_precision: 0,
        }
    }

    #[test]
    fn attributes_follow_semantics_to_parameters() {
        let outputs = [
            element("SV_Position", 0, 0),
            element("COLOR", 0, 1),
            element("TEXCOORD", 0, 2),
            element("TEXCOORD", 3, 4),
        ];
        let exports = [
            ParameterExport {
                kind: 6,
                register: 1,
                parameter: 0,
            },
            ParameterExport {
                kind: 6,
                register: 2,
                parameter: 1,
            },
            ParameterExport {
                kind: 6,
                register: 4,
                parameter: 2,
            },
        ];
        let inputs = [
            element("SV_Position", 0, 0),
            element("TEXCOORD", 3, 1),
            element("TEXCOORD", 0, 2),
            element("NORMAL", 0, 3),
        ];
        let interpolant = |register| Interpolant {
            register,
            mask: 0xF,
            flat: false,
            half: false,
        };
        let sources = pixel_attribute_sources(
            &outputs,
            &exports,
            &inputs,
            &[interpolant(1), interpolant(2), interpolant(3)],
        );
        assert_eq!(sources, [Some(2), Some(1), None]);
    }
}
