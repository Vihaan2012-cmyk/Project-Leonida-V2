//! `ID3D12CommandSignature`: what one `ExecuteIndirect` argument record holds.

use crate::execute::IndirectKind;

/// The argument types of `D3D12_INDIRECT_ARGUMENT_TYPE` that name a draw or dispatch.
const DRAW: u32 = 0;
const DRAW_INDEXED: u32 = 1;
const DISPATCH: u32 = 2;
const DISPATCH_MESH: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndirectLayout {
    /// The one Vulkan call a record maps to; `None` when the signature changes state per record as well.
    pub kind: Option<IndirectKind>,
    pub stride: u32,
}

/// Reads a `D3D12_COMMAND_SIGNATURE_DESC`: `ByteStride`, `NumArgumentDescs`, the array of 16-byte argument
/// descriptions.
///
/// # Safety
/// `desc` is a readable command signature description whose argument array is readable.
pub unsafe fn read_layout(desc: *const u8) -> IndirectLayout {
    // SAFETY: as the caller promises.
    let (stride, count, args) = unsafe {
        (
            *(desc as *const u32),
            *(desc.add(4) as *const u32),
            *(desc.add(8) as *const *const u32),
        )
    };
    let types: Vec<u32> = (0..count as usize)
        .map(|i| {
            // SAFETY: `count` argument descriptions of 16 bytes, the type first.
            unsafe { *(args as *const u8).add(i * 16).cast::<u32>() }
        })
        .collect();
    layout_of(stride, &types)
}

pub fn layout_of(stride: u32, types: &[u32]) -> IndirectLayout {
    let kind = match types {
        [DRAW] => Some((IndirectKind::Draw, 16)),
        [DRAW_INDEXED] => Some((IndirectKind::DrawIndexed, 20)),
        [DISPATCH] => Some((IndirectKind::Dispatch, 12)),
        [DISPATCH_MESH] => Some((IndirectKind::DispatchMesh, 12)),
        _ => None,
    };
    IndirectLayout {
        kind: kind.map(|(k, _)| k),
        stride: if stride != 0 {
            stride
        } else {
            kind.map_or(0, |(_, bytes)| bytes)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_draw_argument_maps_to_one_call_with_its_default_stride() {
        let layout = layout_of(0, &[DRAW_INDEXED]);
        assert_eq!(layout.kind, Some(IndirectKind::DrawIndexed));
        assert_eq!(layout.stride, 20);
        assert_eq!(layout_of(32, &[DISPATCH]).stride, 32);
    }

    #[test]
    fn arguments_that_also_set_state_have_no_single_call() {
        assert_eq!(layout_of(24, &[5, DRAW]).kind, None);
        assert_eq!(layout_of(24, &[]).kind, None);
    }

    #[test]
    fn a_description_is_read_from_memory() {
        let args = [[DRAW, 0, 0, 0], [DRAW, 0, 0, 0]];
        let desc: [u64; 3] = [32 | 1 << 32, args.as_ptr() as u64, 0];
        // SAFETY: `desc` is laid out as the 24-byte description with a pointer to two 16-byte arguments.
        let one = unsafe { read_layout(desc.as_ptr() as *const u8) };
        assert_eq!(one.kind, Some(IndirectKind::Draw));
        assert_eq!(one.stride, 32);
    }
}
