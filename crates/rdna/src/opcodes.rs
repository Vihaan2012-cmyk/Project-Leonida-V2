// Generated from upstream Leonida tools/rdna2_isa_tables.py, itself derived from AMD's machine-readable
// RDNA2 ISA specification (MIT). Regenerate rather than edit.

use crate::table::Field::*;
use crate::table::{OpInfo, Spec};

pub static DS: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "ds_add_u32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "ds_sub_u32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "ds_rsub_u32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "ds_inc_u32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "ds_dec_u32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "ds_min_i32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "ds_max_i32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "ds_min_u32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "ds_max_u32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "ds_and_b32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "ds_or_b32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "ds_xor_b32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "ds_mskor_b32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
                Spec {
                    field: Data1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "ds_write_b32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "ds_write2_b32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
                Spec {
                    field: Data1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "ds_write2st64_b32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
                Spec {
                    field: Data1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x10,
        OpInfo {
            name: "ds_cmpst_b32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
                Spec {
                    field: Data1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11,
        OpInfo {
            name: "ds_cmpst_f32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
                Spec {
                    field: Data1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x12,
        OpInfo {
            name: "ds_min_f32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x13,
        OpInfo {
            name: "ds_max_f32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x14,
        OpInfo {
            name: "ds_nop",
            spec: &[],
        },
    ),
    (
        0x15,
        OpInfo {
            name: "ds_add_f32",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "ds_gws_sema_release_all",
            spec: &[],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "ds_gws_init",
            spec: &[Spec {
                field: Addr,
                regs: 1,
            }],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "ds_gws_sema_v",
            spec: &[],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "ds_gws_sema_br",
            spec: &[Spec {
                field: Addr,
                regs: 1,
            }],
        },
    ),
    (
        0x1c,
        OpInfo {
            name: "ds_gws_sema_p",
            spec: &[],
        },
    ),
    (
        0x1d,
        OpInfo {
            name: "ds_gws_barrier",
            spec: &[Spec {
                field: Addr,
                regs: 1,
            }],
        },
    ),
    (
        0x1e,
        OpInfo {
            name: "ds_write_b8",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "ds_write_b16",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "ds_add_rtn_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "ds_sub_rtn_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x22,
        OpInfo {
            name: "ds_rsub_rtn_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x23,
        OpInfo {
            name: "ds_inc_rtn_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "ds_dec_rtn_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "ds_min_rtn_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x26,
        OpInfo {
            name: "ds_max_rtn_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x27,
        OpInfo {
            name: "ds_min_rtn_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x28,
        OpInfo {
            name: "ds_max_rtn_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x29,
        OpInfo {
            name: "ds_and_rtn_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2a,
        OpInfo {
            name: "ds_or_rtn_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2b,
        OpInfo {
            name: "ds_xor_rtn_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2c,
        OpInfo {
            name: "ds_mskor_rtn_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
                Spec {
                    field: Data1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2d,
        OpInfo {
            name: "ds_wrxchg_rtn_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2e,
        OpInfo {
            name: "ds_wrxchg2_rtn_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
                Spec {
                    field: Data1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2f,
        OpInfo {
            name: "ds_wrxchg2st64_rtn_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
                Spec {
                    field: Data1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x30,
        OpInfo {
            name: "ds_cmpst_rtn_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
                Spec {
                    field: Data1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x31,
        OpInfo {
            name: "ds_cmpst_rtn_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
                Spec {
                    field: Data1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x32,
        OpInfo {
            name: "ds_min_rtn_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x33,
        OpInfo {
            name: "ds_max_rtn_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x34,
        OpInfo {
            name: "ds_wrap_rtn_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
                Spec {
                    field: Data1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x35,
        OpInfo {
            name: "ds_swizzle_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x36,
        OpInfo {
            name: "ds_read_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x37,
        OpInfo {
            name: "ds_read2_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x38,
        OpInfo {
            name: "ds_read2st64_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x39,
        OpInfo {
            name: "ds_read_i8",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3a,
        OpInfo {
            name: "ds_read_u8",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3b,
        OpInfo {
            name: "ds_read_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3c,
        OpInfo {
            name: "ds_read_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3d,
        OpInfo {
            name: "ds_consume",
            spec: &[Spec {
                field: Vdst,
                regs: 1,
            }],
        },
    ),
    (
        0x3e,
        OpInfo {
            name: "ds_append",
            spec: &[Spec {
                field: Vdst,
                regs: 1,
            }],
        },
    ),
    (
        0x3f,
        OpInfo {
            name: "ds_ordered_count",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x40,
        OpInfo {
            name: "ds_add_u64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x41,
        OpInfo {
            name: "ds_sub_u64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x42,
        OpInfo {
            name: "ds_rsub_u64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x43,
        OpInfo {
            name: "ds_inc_u64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x44,
        OpInfo {
            name: "ds_dec_u64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x45,
        OpInfo {
            name: "ds_min_i64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x46,
        OpInfo {
            name: "ds_max_i64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x47,
        OpInfo {
            name: "ds_min_u64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x48,
        OpInfo {
            name: "ds_max_u64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x49,
        OpInfo {
            name: "ds_and_b64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x4a,
        OpInfo {
            name: "ds_or_b64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x4b,
        OpInfo {
            name: "ds_xor_b64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x4c,
        OpInfo {
            name: "ds_mskor_b64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
                Spec {
                    field: Data1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x4d,
        OpInfo {
            name: "ds_write_b64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x4e,
        OpInfo {
            name: "ds_write2_b64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
                Spec {
                    field: Data1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x4f,
        OpInfo {
            name: "ds_write2st64_b64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
                Spec {
                    field: Data1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x50,
        OpInfo {
            name: "ds_cmpst_b64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
                Spec {
                    field: Data1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x51,
        OpInfo {
            name: "ds_cmpst_f64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
                Spec {
                    field: Data1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x52,
        OpInfo {
            name: "ds_min_f64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x53,
        OpInfo {
            name: "ds_max_f64",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x55,
        OpInfo {
            name: "ds_add_rtn_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x60,
        OpInfo {
            name: "ds_add_rtn_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x61,
        OpInfo {
            name: "ds_sub_rtn_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x62,
        OpInfo {
            name: "ds_rsub_rtn_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x63,
        OpInfo {
            name: "ds_inc_rtn_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x64,
        OpInfo {
            name: "ds_dec_rtn_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x65,
        OpInfo {
            name: "ds_min_rtn_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x66,
        OpInfo {
            name: "ds_max_rtn_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x67,
        OpInfo {
            name: "ds_min_rtn_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x68,
        OpInfo {
            name: "ds_max_rtn_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x69,
        OpInfo {
            name: "ds_and_rtn_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x6a,
        OpInfo {
            name: "ds_or_rtn_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x6b,
        OpInfo {
            name: "ds_xor_rtn_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x6c,
        OpInfo {
            name: "ds_mskor_rtn_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
                Spec {
                    field: Data1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x6d,
        OpInfo {
            name: "ds_wrxchg_rtn_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x6e,
        OpInfo {
            name: "ds_wrxchg2_rtn_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 4,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
                Spec {
                    field: Data1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x6f,
        OpInfo {
            name: "ds_wrxchg2st64_rtn_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 4,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
                Spec {
                    field: Data1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x70,
        OpInfo {
            name: "ds_cmpst_rtn_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
                Spec {
                    field: Data1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x71,
        OpInfo {
            name: "ds_cmpst_rtn_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
                Spec {
                    field: Data1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x72,
        OpInfo {
            name: "ds_min_rtn_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x73,
        OpInfo {
            name: "ds_max_rtn_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x76,
        OpInfo {
            name: "ds_read_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x77,
        OpInfo {
            name: "ds_read2_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 4,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x78,
        OpInfo {
            name: "ds_read2st64_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 4,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x7e,
        OpInfo {
            name: "ds_condxchg32_rtn_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa0,
        OpInfo {
            name: "ds_write_b8_d16_hi",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa1,
        OpInfo {
            name: "ds_write_b16_d16_hi",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa2,
        OpInfo {
            name: "ds_read_u8_d16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa3,
        OpInfo {
            name: "ds_read_u8_d16_hi",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa4,
        OpInfo {
            name: "ds_read_i8_d16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa5,
        OpInfo {
            name: "ds_read_i8_d16_hi",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa6,
        OpInfo {
            name: "ds_read_u16_d16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa7,
        OpInfo {
            name: "ds_read_u16_d16_hi",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb0,
        OpInfo {
            name: "ds_write_addtid_b32",
            spec: &[Spec {
                field: Data0,
                regs: 1,
            }],
        },
    ),
    (
        0xb1,
        OpInfo {
            name: "ds_read_addtid_b32",
            spec: &[Spec {
                field: Vdst,
                regs: 1,
            }],
        },
    ),
    (
        0xb2,
        OpInfo {
            name: "ds_permute_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb3,
        OpInfo {
            name: "ds_bpermute_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xde,
        OpInfo {
            name: "ds_write_b96",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 3,
                },
            ],
        },
    ),
    (
        0xdf,
        OpInfo {
            name: "ds_write_b128",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data0,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xfe,
        OpInfo {
            name: "ds_read_b96",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 3,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xff,
        OpInfo {
            name: "ds_read_b128",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 4,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static FLAT: &[(u16, OpInfo)] = &[
    (
        0x8,
        OpInfo {
            name: "flat_load_ubyte",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "flat_load_sbyte",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "flat_load_ushort",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "flat_load_sshort",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "flat_load_dword",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "flat_load_dwordx2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "flat_load_dwordx4",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 4,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "flat_load_dwordx3",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 3,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "flat_store_byte",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "flat_store_byte_d16_hi",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "flat_store_short",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "flat_store_short_d16_hi",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c,
        OpInfo {
            name: "flat_store_dword",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d,
        OpInfo {
            name: "flat_store_dwordx2",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1e,
        OpInfo {
            name: "flat_store_dwordx4",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "flat_store_dwordx3",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 3,
                },
            ],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "flat_load_ubyte_d16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "flat_load_ubyte_d16_hi",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x22,
        OpInfo {
            name: "flat_load_sbyte_d16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x23,
        OpInfo {
            name: "flat_load_sbyte_d16_hi",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "flat_load_short_d16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "flat_load_short_d16_hi",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x30,
        OpInfo {
            name: "flat_atomic_swap",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x31,
        OpInfo {
            name: "flat_atomic_cmpswap",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x32,
        OpInfo {
            name: "flat_atomic_add",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x33,
        OpInfo {
            name: "flat_atomic_sub",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x35,
        OpInfo {
            name: "flat_atomic_smin",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x36,
        OpInfo {
            name: "flat_atomic_umin",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x37,
        OpInfo {
            name: "flat_atomic_smax",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x38,
        OpInfo {
            name: "flat_atomic_umax",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x39,
        OpInfo {
            name: "flat_atomic_and",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3a,
        OpInfo {
            name: "flat_atomic_or",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3b,
        OpInfo {
            name: "flat_atomic_xor",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3c,
        OpInfo {
            name: "flat_atomic_inc",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3d,
        OpInfo {
            name: "flat_atomic_dec",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3e,
        OpInfo {
            name: "flat_atomic_fcmpswap",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3f,
        OpInfo {
            name: "flat_atomic_fmin",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x40,
        OpInfo {
            name: "flat_atomic_fmax",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x50,
        OpInfo {
            name: "flat_atomic_swap_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x51,
        OpInfo {
            name: "flat_atomic_cmpswap_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x52,
        OpInfo {
            name: "flat_atomic_add_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x53,
        OpInfo {
            name: "flat_atomic_sub_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x55,
        OpInfo {
            name: "flat_atomic_smin_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x56,
        OpInfo {
            name: "flat_atomic_umin_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x57,
        OpInfo {
            name: "flat_atomic_smax_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x58,
        OpInfo {
            name: "flat_atomic_umax_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x59,
        OpInfo {
            name: "flat_atomic_and_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x5a,
        OpInfo {
            name: "flat_atomic_or_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x5b,
        OpInfo {
            name: "flat_atomic_xor_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x5c,
        OpInfo {
            name: "flat_atomic_inc_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x5d,
        OpInfo {
            name: "flat_atomic_dec_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x5e,
        OpInfo {
            name: "flat_atomic_fcmpswap_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x5f,
        OpInfo {
            name: "flat_atomic_fmin_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x60,
        OpInfo {
            name: "flat_atomic_fmax_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
            ],
        },
    ),
];

pub static GLOBAL: &[(u16, OpInfo)] = &[
    (
        0x8,
        OpInfo {
            name: "global_load_ubyte",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "global_load_sbyte",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "global_load_ushort",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "global_load_sshort",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "global_load_dword",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "global_load_dwordx2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "global_load_dwordx4",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 4,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "global_load_dwordx3",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 3,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x16,
        OpInfo {
            name: "global_load_dword_addtid",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x17,
        OpInfo {
            name: "global_store_dword_addtid",
            spec: &[
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "global_store_byte",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "global_store_byte_d16_hi",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "global_store_short",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "global_store_short_d16_hi",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1c,
        OpInfo {
            name: "global_store_dword",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1d,
        OpInfo {
            name: "global_store_dwordx2",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1e,
        OpInfo {
            name: "global_store_dwordx4",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 4,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "global_store_dwordx3",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 3,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "global_load_ubyte_d16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "global_load_ubyte_d16_hi",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x22,
        OpInfo {
            name: "global_load_sbyte_d16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x23,
        OpInfo {
            name: "global_load_sbyte_d16_hi",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "global_load_short_d16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "global_load_short_d16_hi",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x30,
        OpInfo {
            name: "global_atomic_swap",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x31,
        OpInfo {
            name: "global_atomic_cmpswap",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x32,
        OpInfo {
            name: "global_atomic_add",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x33,
        OpInfo {
            name: "global_atomic_sub",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x34,
        OpInfo {
            name: "global_atomic_csub",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x35,
        OpInfo {
            name: "global_atomic_smin",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x36,
        OpInfo {
            name: "global_atomic_umin",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x37,
        OpInfo {
            name: "global_atomic_smax",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x38,
        OpInfo {
            name: "global_atomic_umax",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x39,
        OpInfo {
            name: "global_atomic_and",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3a,
        OpInfo {
            name: "global_atomic_or",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3b,
        OpInfo {
            name: "global_atomic_xor",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3c,
        OpInfo {
            name: "global_atomic_inc",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3d,
        OpInfo {
            name: "global_atomic_dec",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3e,
        OpInfo {
            name: "global_atomic_fcmpswap",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3f,
        OpInfo {
            name: "global_atomic_fmin",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x40,
        OpInfo {
            name: "global_atomic_fmax",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x50,
        OpInfo {
            name: "global_atomic_swap_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x51,
        OpInfo {
            name: "global_atomic_cmpswap_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 4,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x52,
        OpInfo {
            name: "global_atomic_add_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x53,
        OpInfo {
            name: "global_atomic_sub_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x55,
        OpInfo {
            name: "global_atomic_smin_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x56,
        OpInfo {
            name: "global_atomic_umin_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x57,
        OpInfo {
            name: "global_atomic_smax_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x58,
        OpInfo {
            name: "global_atomic_umax_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x59,
        OpInfo {
            name: "global_atomic_and_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x5a,
        OpInfo {
            name: "global_atomic_or_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x5b,
        OpInfo {
            name: "global_atomic_xor_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x5c,
        OpInfo {
            name: "global_atomic_inc_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x5d,
        OpInfo {
            name: "global_atomic_dec_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x5e,
        OpInfo {
            name: "global_atomic_fcmpswap_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 4,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x5f,
        OpInfo {
            name: "global_atomic_fmin_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x60,
        OpInfo {
            name: "global_atomic_fmax_x2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 2,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 2,
                },
            ],
        },
    ),
];

pub static MIMG: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "image_load",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "image_load_mip",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "image_load_pck",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "image_load_pck_sgn",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "image_load_mip_pck",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "image_load_mip_pck_sgn",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "image_store",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "image_store_mip",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "image_store_pck",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "image_store_mip_pck",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "image_get_resinfo",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 1,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "image_atomic_swap",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x10,
        OpInfo {
            name: "image_atomic_cmpswap",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x11,
        OpInfo {
            name: "image_atomic_add",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x12,
        OpInfo {
            name: "image_atomic_sub",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x14,
        OpInfo {
            name: "image_atomic_smin",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x15,
        OpInfo {
            name: "image_atomic_umin",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x16,
        OpInfo {
            name: "image_atomic_smax",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x17,
        OpInfo {
            name: "image_atomic_umax",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "image_atomic_and",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "image_atomic_or",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "image_atomic_xor",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "image_atomic_inc",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x1c,
        OpInfo {
            name: "image_atomic_dec",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x1d,
        OpInfo {
            name: "image_atomic_fcmpswap",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x1e,
        OpInfo {
            name: "image_atomic_fmin",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "image_atomic_fmax",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "image_sample",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 3,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "image_sample_cl",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x22,
        OpInfo {
            name: "image_sample_d",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 9,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x23,
        OpInfo {
            name: "image_sample_d_cl",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 10,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "image_sample_l",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "image_sample_b",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x26,
        OpInfo {
            name: "image_sample_b_cl",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x27,
        OpInfo {
            name: "image_sample_lz",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 3,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x28,
        OpInfo {
            name: "image_sample_c",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x29,
        OpInfo {
            name: "image_sample_c_cl",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x2a,
        OpInfo {
            name: "image_sample_c_d",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 10,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x2b,
        OpInfo {
            name: "image_sample_c_d_cl",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 11,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x2c,
        OpInfo {
            name: "image_sample_c_l",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x2d,
        OpInfo {
            name: "image_sample_c_b",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x2e,
        OpInfo {
            name: "image_sample_c_b_cl",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 6,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x2f,
        OpInfo {
            name: "image_sample_c_lz",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x30,
        OpInfo {
            name: "image_sample_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x31,
        OpInfo {
            name: "image_sample_cl_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x32,
        OpInfo {
            name: "image_sample_d_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 10,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x33,
        OpInfo {
            name: "image_sample_d_cl_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 11,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x34,
        OpInfo {
            name: "image_sample_l_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x35,
        OpInfo {
            name: "image_sample_b_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x36,
        OpInfo {
            name: "image_sample_b_cl_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 6,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x37,
        OpInfo {
            name: "image_sample_lz_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x38,
        OpInfo {
            name: "image_sample_c_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x39,
        OpInfo {
            name: "image_sample_c_cl_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 6,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x3a,
        OpInfo {
            name: "image_sample_c_d_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 11,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x3b,
        OpInfo {
            name: "image_sample_c_d_cl_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 12,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x3c,
        OpInfo {
            name: "image_sample_c_l_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 6,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x3d,
        OpInfo {
            name: "image_sample_c_b_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 6,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x3e,
        OpInfo {
            name: "image_sample_c_b_cl_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 7,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x3f,
        OpInfo {
            name: "image_sample_c_lz_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x40,
        OpInfo {
            name: "image_gather4",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 3,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x41,
        OpInfo {
            name: "image_gather4_cl",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x42,
        OpInfo {
            name: "image_load_by2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x43,
        OpInfo {
            name: "image_load_by4",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x44,
        OpInfo {
            name: "image_gather4_l",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x45,
        OpInfo {
            name: "image_gather4_b",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x46,
        OpInfo {
            name: "image_gather4_b_cl",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x47,
        OpInfo {
            name: "image_gather4_lz",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 3,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x48,
        OpInfo {
            name: "image_gather4_c",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x49,
        OpInfo {
            name: "image_gather4_c_cl",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x4a,
        OpInfo {
            name: "image_load_mip_by2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x4b,
        OpInfo {
            name: "image_load_mip_by4",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x4c,
        OpInfo {
            name: "image_gather4_c_l",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x4d,
        OpInfo {
            name: "image_gather4_c_b",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x4e,
        OpInfo {
            name: "image_gather4_c_b_cl",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 6,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x4f,
        OpInfo {
            name: "image_gather4_c_lz",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x50,
        OpInfo {
            name: "image_gather4_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x51,
        OpInfo {
            name: "image_gather4_cl_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x52,
        OpInfo {
            name: "image_store_by2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x53,
        OpInfo {
            name: "image_store_by4",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x54,
        OpInfo {
            name: "image_gather4_l_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x55,
        OpInfo {
            name: "image_gather4_b_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x56,
        OpInfo {
            name: "image_gather4_b_cl_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 6,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x57,
        OpInfo {
            name: "image_gather4_lz_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x58,
        OpInfo {
            name: "image_gather4_c_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x59,
        OpInfo {
            name: "image_gather4_c_cl_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 6,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x5a,
        OpInfo {
            name: "image_store_mip_by2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x5b,
        OpInfo {
            name: "image_store_mip_by4",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x5c,
        OpInfo {
            name: "image_gather4_c_l_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 6,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x5d,
        OpInfo {
            name: "image_gather4_c_b_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 6,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x5e,
        OpInfo {
            name: "image_gather4_c_b_cl_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 7,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x5f,
        OpInfo {
            name: "image_gather4_c_lz_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 5,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x60,
        OpInfo {
            name: "image_get_lod",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 3,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x61,
        OpInfo {
            name: "image_gather4h",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 3,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x62,
        OpInfo {
            name: "image_gather4h_pck",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 3,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x63,
        OpInfo {
            name: "image_gather8h_pck",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 3,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x68,
        OpInfo {
            name: "image_sample_cd",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 9,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x69,
        OpInfo {
            name: "image_sample_cd_cl",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 10,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x6a,
        OpInfo {
            name: "image_sample_c_cd",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 10,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x6b,
        OpInfo {
            name: "image_sample_c_cd_cl",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 11,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x6c,
        OpInfo {
            name: "image_sample_cd_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 10,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x6d,
        OpInfo {
            name: "image_sample_cd_cl_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 11,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x6e,
        OpInfo {
            name: "image_sample_c_cd_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 11,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x6f,
        OpInfo {
            name: "image_sample_c_cd_cl_o",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 12,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x70,
        OpInfo {
            name: "image_load_pck2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x71,
        OpInfo {
            name: "image_load_pck4",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x73,
        OpInfo {
            name: "image_load_mip_pck2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x74,
        OpInfo {
            name: "image_load_mip_pck4",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x76,
        OpInfo {
            name: "image_store_pck2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x77,
        OpInfo {
            name: "image_store_pck4",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x79,
        OpInfo {
            name: "image_store_mip_pck2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x7a,
        OpInfo {
            name: "image_store_mip_pck4",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0x80,
        OpInfo {
            name: "image_msaa_load",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 4,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
            ],
        },
    ),
    (
        0xa2,
        OpInfo {
            name: "image_sample_d_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 7,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xa3,
        OpInfo {
            name: "image_sample_d_cl_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 8,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xaa,
        OpInfo {
            name: "image_sample_c_d_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 8,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xab,
        OpInfo {
            name: "image_sample_c_d_cl_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 9,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xb2,
        OpInfo {
            name: "image_sample_d_o_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 8,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xb3,
        OpInfo {
            name: "image_sample_d_cl_o_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 9,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xba,
        OpInfo {
            name: "image_sample_c_d_o_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 9,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xbb,
        OpInfo {
            name: "image_sample_c_d_cl_o_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 10,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xe6,
        OpInfo {
            name: "image_bvh_intersect_ray",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 11,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xe7,
        OpInfo {
            name: "image_bvh64_intersect_ray",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 12,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xe8,
        OpInfo {
            name: "image_sample_cd_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 7,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xe9,
        OpInfo {
            name: "image_sample_cd_cl_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 8,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xea,
        OpInfo {
            name: "image_sample_c_cd_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 8,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xeb,
        OpInfo {
            name: "image_sample_c_cd_cl_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 9,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xec,
        OpInfo {
            name: "image_sample_cd_o_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 8,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xed,
        OpInfo {
            name: "image_sample_cd_cl_o_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 9,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xee,
        OpInfo {
            name: "image_sample_c_cd_o_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 9,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0xef,
        OpInfo {
            name: "image_sample_c_cd_cl_o_g16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 10,
                },
                Spec {
                    field: Srsrc,
                    regs: 8,
                },
                Spec {
                    field: Ssamp,
                    regs: 4,
                },
            ],
        },
    ),
];

pub static MTBUF: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "tbuffer_load_format_x",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "tbuffer_load_format_xy",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "tbuffer_load_format_xyz",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 3,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "tbuffer_load_format_xyzw",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "tbuffer_store_format_x",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "tbuffer_store_format_xy",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "tbuffer_store_format_xyz",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 3,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "tbuffer_store_format_xyzw",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "tbuffer_load_format_d16_x",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "tbuffer_load_format_d16_xy",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "tbuffer_load_format_d16_xyz",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "tbuffer_load_format_d16_xyzw",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "tbuffer_store_format_d16_x",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "tbuffer_store_format_d16_xy",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "tbuffer_store_format_d16_xyz",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "tbuffer_store_format_d16_xyzw",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static MUBUF: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "buffer_load_format_x",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "buffer_load_format_xy",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "buffer_load_format_xyz",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 3,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "buffer_load_format_xyzw",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "buffer_store_format_x",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "buffer_store_format_xy",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "buffer_store_format_xyz",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 3,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "buffer_store_format_xyzw",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "buffer_load_ubyte",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "buffer_load_sbyte",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "buffer_load_ushort",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "buffer_load_sshort",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "buffer_load_dword",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "buffer_load_dwordx2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "buffer_load_dwordx4",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "buffer_load_dwordx3",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 3,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "buffer_store_byte",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "buffer_store_byte_d16_hi",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "buffer_store_short",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "buffer_store_short_d16_hi",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c,
        OpInfo {
            name: "buffer_store_dword",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d,
        OpInfo {
            name: "buffer_store_dwordx2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1e,
        OpInfo {
            name: "buffer_store_dwordx4",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "buffer_store_dwordx3",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 3,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "buffer_load_ubyte_d16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "buffer_load_ubyte_d16_hi",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x22,
        OpInfo {
            name: "buffer_load_sbyte_d16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x23,
        OpInfo {
            name: "buffer_load_sbyte_d16_hi",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "buffer_load_short_d16",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "buffer_load_short_d16_hi",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x26,
        OpInfo {
            name: "buffer_load_format_d16_hi_x",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x27,
        OpInfo {
            name: "buffer_store_format_d16_hi_x",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x30,
        OpInfo {
            name: "buffer_atomic_swap",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x31,
        OpInfo {
            name: "buffer_atomic_cmpswap",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x32,
        OpInfo {
            name: "buffer_atomic_add",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x33,
        OpInfo {
            name: "buffer_atomic_sub",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x34,
        OpInfo {
            name: "buffer_atomic_csub",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x35,
        OpInfo {
            name: "buffer_atomic_smin",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x36,
        OpInfo {
            name: "buffer_atomic_umin",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x37,
        OpInfo {
            name: "buffer_atomic_smax",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x38,
        OpInfo {
            name: "buffer_atomic_umax",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x39,
        OpInfo {
            name: "buffer_atomic_and",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3a,
        OpInfo {
            name: "buffer_atomic_or",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3b,
        OpInfo {
            name: "buffer_atomic_xor",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3c,
        OpInfo {
            name: "buffer_atomic_inc",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3d,
        OpInfo {
            name: "buffer_atomic_dec",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3e,
        OpInfo {
            name: "buffer_atomic_fcmpswap",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3f,
        OpInfo {
            name: "buffer_atomic_fmin",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x40,
        OpInfo {
            name: "buffer_atomic_fmax",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x50,
        OpInfo {
            name: "buffer_atomic_swap_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x51,
        OpInfo {
            name: "buffer_atomic_cmpswap_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x52,
        OpInfo {
            name: "buffer_atomic_add_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x53,
        OpInfo {
            name: "buffer_atomic_sub_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x55,
        OpInfo {
            name: "buffer_atomic_smin_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x56,
        OpInfo {
            name: "buffer_atomic_umin_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x57,
        OpInfo {
            name: "buffer_atomic_smax_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x58,
        OpInfo {
            name: "buffer_atomic_umax_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x59,
        OpInfo {
            name: "buffer_atomic_and_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5a,
        OpInfo {
            name: "buffer_atomic_or_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5b,
        OpInfo {
            name: "buffer_atomic_xor_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5c,
        OpInfo {
            name: "buffer_atomic_inc_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5d,
        OpInfo {
            name: "buffer_atomic_dec_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5e,
        OpInfo {
            name: "buffer_atomic_fcmpswap_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 4,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5f,
        OpInfo {
            name: "buffer_atomic_fmin_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x60,
        OpInfo {
            name: "buffer_atomic_fmax_x2",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x71,
        OpInfo {
            name: "buffer_gl0_inv",
            spec: &[],
        },
    ),
    (
        0x72,
        OpInfo {
            name: "buffer_gl1_inv",
            spec: &[],
        },
    ),
    (
        0x80,
        OpInfo {
            name: "buffer_load_format_d16_x",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x81,
        OpInfo {
            name: "buffer_load_format_d16_xy",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x82,
        OpInfo {
            name: "buffer_load_format_d16_xyz",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x83,
        OpInfo {
            name: "buffer_load_format_d16_xyzw",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x84,
        OpInfo {
            name: "buffer_store_format_d16_x",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x85,
        OpInfo {
            name: "buffer_store_format_d16_xy",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 1,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x86,
        OpInfo {
            name: "buffer_store_format_d16_xyz",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x87,
        OpInfo {
            name: "buffer_store_format_d16_xyzw",
            spec: &[
                Spec {
                    field: Vdata,
                    regs: 2,
                },
                Spec {
                    field: Vaddr,
                    regs: 2,
                },
                Spec {
                    field: Srsrc,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static SCRATCH: &[(u16, OpInfo)] = &[
    (
        0x8,
        OpInfo {
            name: "scratch_load_ubyte",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "scratch_load_sbyte",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "scratch_load_ushort",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "scratch_load_sshort",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "scratch_load_dword",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "scratch_load_dwordx2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "scratch_load_dwordx4",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 4,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "scratch_load_dwordx3",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 3,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "scratch_store_byte",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "scratch_store_byte_d16_hi",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "scratch_store_short",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "scratch_store_short_d16_hi",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c,
        OpInfo {
            name: "scratch_store_dword",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d,
        OpInfo {
            name: "scratch_store_dwordx2",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data,
                    regs: 2,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1e,
        OpInfo {
            name: "scratch_store_dwordx4",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data,
                    regs: 4,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "scratch_store_dwordx3",
            spec: &[
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Data,
                    regs: 3,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "scratch_load_ubyte_d16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "scratch_load_ubyte_d16_hi",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x22,
        OpInfo {
            name: "scratch_load_sbyte_d16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x23,
        OpInfo {
            name: "scratch_load_sbyte_d16_hi",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "scratch_load_short_d16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "scratch_load_short_d16_hi",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Addr,
                    regs: 1,
                },
                Spec {
                    field: Saddr,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static SMEM: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "s_load_dword",
            spec: &[
                Spec {
                    field: Sdata,
                    regs: 1,
                },
                Spec {
                    field: Sbase,
                    regs: 2,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "s_load_dwordx2",
            spec: &[
                Spec {
                    field: Sdata,
                    regs: 2,
                },
                Spec {
                    field: Sbase,
                    regs: 2,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "s_load_dwordx4",
            spec: &[
                Spec {
                    field: Sdata,
                    regs: 4,
                },
                Spec {
                    field: Sbase,
                    regs: 2,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "s_load_dwordx8",
            spec: &[
                Spec {
                    field: Sdata,
                    regs: 8,
                },
                Spec {
                    field: Sbase,
                    regs: 2,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "s_load_dwordx16",
            spec: &[
                Spec {
                    field: Sdata,
                    regs: 16,
                },
                Spec {
                    field: Sbase,
                    regs: 2,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "s_buffer_load_dword",
            spec: &[
                Spec {
                    field: Sdata,
                    regs: 1,
                },
                Spec {
                    field: Sbase,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "s_buffer_load_dwordx2",
            spec: &[
                Spec {
                    field: Sdata,
                    regs: 2,
                },
                Spec {
                    field: Sbase,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "s_buffer_load_dwordx4",
            spec: &[
                Spec {
                    field: Sdata,
                    regs: 4,
                },
                Spec {
                    field: Sbase,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "s_buffer_load_dwordx8",
            spec: &[
                Spec {
                    field: Sdata,
                    regs: 8,
                },
                Spec {
                    field: Sbase,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "s_buffer_load_dwordx16",
            spec: &[
                Spec {
                    field: Sdata,
                    regs: 16,
                },
                Spec {
                    field: Sbase,
                    regs: 4,
                },
                Spec {
                    field: Soffset,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "s_gl1_inv",
            spec: &[],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "s_dcache_inv",
            spec: &[],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "s_memtime",
            spec: &[Spec {
                field: Sdata,
                regs: 2,
            }],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "s_memrealtime",
            spec: &[Spec {
                field: Sdata,
                regs: 2,
            }],
        },
    ),
    (
        0x2a,
        OpInfo {
            name: "s_get_waveid_in_workgroup",
            spec: &[Spec {
                field: Sdata,
                regs: 1,
            }],
        },
    ),
];

pub static SOP1: &[(u16, OpInfo)] = &[
    (
        0x3,
        OpInfo {
            name: "s_mov_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "s_mov_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "s_cmov_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "s_cmov_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "s_not_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "s_not_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "s_wqm_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "s_wqm_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "s_brev_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "s_brev_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "s_bcnt0_i32_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "s_bcnt0_i32_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "s_bcnt1_i32_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x10,
        OpInfo {
            name: "s_bcnt1_i32_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x11,
        OpInfo {
            name: "s_ff0_i32_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x12,
        OpInfo {
            name: "s_ff0_i32_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x13,
        OpInfo {
            name: "s_ff1_i32_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x14,
        OpInfo {
            name: "s_ff1_i32_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x15,
        OpInfo {
            name: "s_flbit_i32_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x16,
        OpInfo {
            name: "s_flbit_i32_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x17,
        OpInfo {
            name: "s_flbit_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "s_flbit_i32_i64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "s_sext_i32_i8",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "s_sext_i32_i16",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "s_bitset0_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c,
        OpInfo {
            name: "s_bitset0_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d,
        OpInfo {
            name: "s_bitset1_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1e,
        OpInfo {
            name: "s_bitset1_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "s_getpc_b64",
            spec: &[Spec {
                field: Sdst,
                regs: 2,
            }],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "s_setpc_b64",
            spec: &[Spec {
                field: Ssrc0,
                regs: 2,
            }],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "s_swappc_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x22,
        OpInfo {
            name: "s_rfe_b64",
            spec: &[Spec {
                field: Ssrc0,
                regs: 2,
            }],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "s_and_saveexec_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "s_or_saveexec_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x26,
        OpInfo {
            name: "s_xor_saveexec_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x27,
        OpInfo {
            name: "s_andn2_saveexec_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x28,
        OpInfo {
            name: "s_orn2_saveexec_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x29,
        OpInfo {
            name: "s_nand_saveexec_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2a,
        OpInfo {
            name: "s_nor_saveexec_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2b,
        OpInfo {
            name: "s_xnor_saveexec_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2c,
        OpInfo {
            name: "s_quadmask_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2d,
        OpInfo {
            name: "s_quadmask_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2e,
        OpInfo {
            name: "s_movrels_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2f,
        OpInfo {
            name: "s_movrels_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x30,
        OpInfo {
            name: "s_movreld_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x31,
        OpInfo {
            name: "s_movreld_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x34,
        OpInfo {
            name: "s_abs_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x37,
        OpInfo {
            name: "s_andn1_saveexec_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x38,
        OpInfo {
            name: "s_orn1_saveexec_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x39,
        OpInfo {
            name: "s_andn1_wrexec_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3a,
        OpInfo {
            name: "s_andn2_wrexec_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3b,
        OpInfo {
            name: "s_bitreplicate_b64_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3c,
        OpInfo {
            name: "s_and_saveexec_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3d,
        OpInfo {
            name: "s_or_saveexec_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3e,
        OpInfo {
            name: "s_xor_saveexec_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3f,
        OpInfo {
            name: "s_andn2_saveexec_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x40,
        OpInfo {
            name: "s_orn2_saveexec_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x41,
        OpInfo {
            name: "s_nand_saveexec_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x42,
        OpInfo {
            name: "s_nor_saveexec_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x43,
        OpInfo {
            name: "s_xnor_saveexec_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x44,
        OpInfo {
            name: "s_andn1_saveexec_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x45,
        OpInfo {
            name: "s_orn1_saveexec_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x46,
        OpInfo {
            name: "s_andn1_wrexec_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x47,
        OpInfo {
            name: "s_andn2_wrexec_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x49,
        OpInfo {
            name: "s_movrelsd_2_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static SOP2: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "s_add_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "s_sub_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "s_add_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "s_sub_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "s_addc_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "s_subb_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "s_min_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "s_min_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "s_max_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "s_max_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "s_cselect_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "s_cselect_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "s_and_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "s_and_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x10,
        OpInfo {
            name: "s_or_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11,
        OpInfo {
            name: "s_or_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x12,
        OpInfo {
            name: "s_xor_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x13,
        OpInfo {
            name: "s_xor_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x14,
        OpInfo {
            name: "s_andn2_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x15,
        OpInfo {
            name: "s_andn2_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x16,
        OpInfo {
            name: "s_orn2_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x17,
        OpInfo {
            name: "s_orn2_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "s_nand_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "s_nand_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "s_nor_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "s_nor_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1c,
        OpInfo {
            name: "s_xnor_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d,
        OpInfo {
            name: "s_xnor_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1e,
        OpInfo {
            name: "s_lshl_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "s_lshl_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "s_lshr_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "s_lshr_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x22,
        OpInfo {
            name: "s_ashr_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x23,
        OpInfo {
            name: "s_ashr_i64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "s_bfm_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "s_bfm_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x26,
        OpInfo {
            name: "s_mul_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x27,
        OpInfo {
            name: "s_bfe_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x28,
        OpInfo {
            name: "s_bfe_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x29,
        OpInfo {
            name: "s_bfe_u64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2a,
        OpInfo {
            name: "s_bfe_i64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2c,
        OpInfo {
            name: "s_absdiff_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2e,
        OpInfo {
            name: "s_lshl1_add_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2f,
        OpInfo {
            name: "s_lshl2_add_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x30,
        OpInfo {
            name: "s_lshl3_add_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x31,
        OpInfo {
            name: "s_lshl4_add_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x32,
        OpInfo {
            name: "s_pack_ll_b32_b16",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x33,
        OpInfo {
            name: "s_pack_lh_b32_b16",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x34,
        OpInfo {
            name: "s_pack_hh_b32_b16",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x35,
        OpInfo {
            name: "s_mul_hi_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x36,
        OpInfo {
            name: "s_mul_hi_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static SOPC: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "s_cmp_eq_i32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "s_cmp_lg_i32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "s_cmp_gt_i32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "s_cmp_ge_i32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "s_cmp_lt_i32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "s_cmp_le_i32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "s_cmp_eq_u32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "s_cmp_lg_u32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "s_cmp_gt_u32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "s_cmp_ge_u32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "s_cmp_lt_u32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "s_cmp_le_u32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "s_bitcmp0_b32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "s_bitcmp1_b32",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 1,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "s_bitcmp0_b64",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "s_bitcmp1_b64",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x12,
        OpInfo {
            name: "s_cmp_eq_u64",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x13,
        OpInfo {
            name: "s_cmp_lg_u64",
            spec: &[
                Spec {
                    field: Ssrc0,
                    regs: 2,
                },
                Spec {
                    field: Ssrc1,
                    regs: 2,
                },
            ],
        },
    ),
];

pub static SOPK: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "s_movk_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "s_version",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "s_cmovk_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "s_cmpk_eq_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "s_cmpk_lg_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "s_cmpk_gt_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "s_cmpk_ge_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "s_cmpk_lt_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "s_cmpk_le_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "s_cmpk_eq_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "s_cmpk_lg_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "s_cmpk_gt_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "s_cmpk_ge_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "s_cmpk_lt_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "s_cmpk_le_u32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "s_addk_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x10,
        OpInfo {
            name: "s_mulk_i32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x12,
        OpInfo {
            name: "s_getreg_b32",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x13,
        OpInfo {
            name: "s_setreg_b32",
            spec: &[
                Spec {
                    field: Simm16,
                    regs: 1,
                },
                Spec {
                    field: Sdst,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x15,
        OpInfo {
            name: "s_setreg_imm32_b32",
            spec: &[
                Spec {
                    field: Simm16,
                    regs: 1,
                },
                Spec {
                    field: Literal,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x16,
        OpInfo {
            name: "s_call_b64",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 2,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x17,
        OpInfo {
            name: "s_waitcnt_vscnt",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "s_waitcnt_vmcnt",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "s_waitcnt_expcnt",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "s_waitcnt_lgkmcnt",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "s_subvector_loop_begin",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c,
        OpInfo {
            name: "s_subvector_loop_end",
            spec: &[
                Spec {
                    field: Sdst,
                    regs: 1,
                },
                Spec {
                    field: Simm16,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static SOPP: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "s_nop",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "s_endpgm",
            spec: &[],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "s_branch",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "s_wakeup",
            spec: &[],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "s_cbranch_scc0",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "s_cbranch_scc1",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "s_cbranch_vccz",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "s_cbranch_vccnz",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "s_cbranch_execz",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "s_cbranch_execnz",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "s_barrier",
            spec: &[],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "s_setkill",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "s_waitcnt",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "s_sethalt",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "s_sleep",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "s_setprio",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x10,
        OpInfo {
            name: "s_sendmsg",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x11,
        OpInfo {
            name: "s_sendmsghalt",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x12,
        OpInfo {
            name: "s_trap",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x13,
        OpInfo {
            name: "s_icache_inv",
            spec: &[],
        },
    ),
    (
        0x14,
        OpInfo {
            name: "s_incperflevel",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x15,
        OpInfo {
            name: "s_decperflevel",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x16,
        OpInfo {
            name: "s_ttracedata",
            spec: &[],
        },
    ),
    (
        0x17,
        OpInfo {
            name: "s_cbranch_cdbgsys",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "s_cbranch_cdbguser",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "s_cbranch_cdbgsys_or_user",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "s_cbranch_cdbgsys_and_user",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "s_endpgm_saved",
            spec: &[],
        },
    ),
    (
        0x1e,
        OpInfo {
            name: "s_endpgm_ordered_ps_done",
            spec: &[],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "s_code_end",
            spec: &[],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "s_inst_prefetch",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "s_clause",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x23,
        OpInfo {
            name: "s_waitcnt_depctr",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "s_round_mode",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "s_denorm_mode",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
    (
        0x28,
        OpInfo {
            name: "s_ttracedata_imm",
            spec: &[Spec {
                field: Simm16,
                regs: 1,
            }],
        },
    ),
];

pub static VINTRP: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "v_interp_p1_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Vsrc,
                    regs: 1,
                },
                Spec {
                    field: Attr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "v_interp_p2_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Vsrc,
                    regs: 1,
                },
                Spec {
                    field: Attr,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "v_interp_mov_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Vsrc,
                    regs: 1,
                },
                Spec {
                    field: Attr,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static VOP1: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "v_nop",
            spec: &[],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "v_mov_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "v_readfirstlane_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "v_cvt_i32_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "v_cvt_f64_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "v_cvt_f32_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "v_cvt_f32_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "v_cvt_u32_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "v_cvt_i32_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "v_cvt_f16_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "v_cvt_f32_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "v_cvt_rpi_i32_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "v_cvt_flr_i32_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "v_cvt_off_f32_i4",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "v_cvt_f32_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x10,
        OpInfo {
            name: "v_cvt_f64_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11,
        OpInfo {
            name: "v_cvt_f32_ubyte0",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x12,
        OpInfo {
            name: "v_cvt_f32_ubyte1",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x13,
        OpInfo {
            name: "v_cvt_f32_ubyte2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x14,
        OpInfo {
            name: "v_cvt_f32_ubyte3",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x15,
        OpInfo {
            name: "v_cvt_u32_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x16,
        OpInfo {
            name: "v_cvt_f64_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x17,
        OpInfo {
            name: "v_trunc_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "v_ceil_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "v_rndne_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "v_floor_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "v_pipeflush",
            spec: &[],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "v_fract_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "v_trunc_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x22,
        OpInfo {
            name: "v_ceil_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x23,
        OpInfo {
            name: "v_rndne_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "v_floor_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "v_exp_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x27,
        OpInfo {
            name: "v_log_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2a,
        OpInfo {
            name: "v_rcp_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2b,
        OpInfo {
            name: "v_rcp_iflag_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2e,
        OpInfo {
            name: "v_rsq_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2f,
        OpInfo {
            name: "v_rcp_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x31,
        OpInfo {
            name: "v_rsq_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x33,
        OpInfo {
            name: "v_sqrt_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x34,
        OpInfo {
            name: "v_sqrt_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x35,
        OpInfo {
            name: "v_sin_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x36,
        OpInfo {
            name: "v_cos_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x37,
        OpInfo {
            name: "v_not_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x38,
        OpInfo {
            name: "v_bfrev_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x39,
        OpInfo {
            name: "v_ffbh_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3a,
        OpInfo {
            name: "v_ffbl_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3b,
        OpInfo {
            name: "v_ffbh_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3c,
        OpInfo {
            name: "v_frexp_exp_i32_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3d,
        OpInfo {
            name: "v_frexp_mant_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3e,
        OpInfo {
            name: "v_fract_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3f,
        OpInfo {
            name: "v_frexp_exp_i32_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x40,
        OpInfo {
            name: "v_frexp_mant_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x41,
        OpInfo {
            name: "v_clrexcp",
            spec: &[],
        },
    ),
    (
        0x42,
        OpInfo {
            name: "v_movreld_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x43,
        OpInfo {
            name: "v_movrels_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x44,
        OpInfo {
            name: "v_movrelsd_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x48,
        OpInfo {
            name: "v_movrelsd_2_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x50,
        OpInfo {
            name: "v_cvt_f16_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x51,
        OpInfo {
            name: "v_cvt_f16_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x52,
        OpInfo {
            name: "v_cvt_u16_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x53,
        OpInfo {
            name: "v_cvt_i16_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x54,
        OpInfo {
            name: "v_rcp_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x55,
        OpInfo {
            name: "v_sqrt_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x56,
        OpInfo {
            name: "v_rsq_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x57,
        OpInfo {
            name: "v_log_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x58,
        OpInfo {
            name: "v_exp_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x59,
        OpInfo {
            name: "v_frexp_mant_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5a,
        OpInfo {
            name: "v_frexp_exp_i16_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5b,
        OpInfo {
            name: "v_floor_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5c,
        OpInfo {
            name: "v_ceil_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5d,
        OpInfo {
            name: "v_trunc_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5e,
        OpInfo {
            name: "v_rndne_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5f,
        OpInfo {
            name: "v_fract_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x60,
        OpInfo {
            name: "v_sin_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x61,
        OpInfo {
            name: "v_cos_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x62,
        OpInfo {
            name: "v_sat_pk_u8_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x63,
        OpInfo {
            name: "v_cvt_norm_i16_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x64,
        OpInfo {
            name: "v_cvt_norm_u16_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x65,
        OpInfo {
            name: "v_swap_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x68,
        OpInfo {
            name: "v_swaprel_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static VOP2: &[(u16, OpInfo)] = &[
    (
        0x1,
        OpInfo {
            name: "v_cndmask_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
                Spec {
                    field: Vcc,
                    regs: -1,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "v_dot2c_f32_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "v_add_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "v_sub_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "v_subrev_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "v_fmac_legacy_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "v_mul_legacy_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "v_mul_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "v_mul_i32_i24",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "v_mul_hi_i32_i24",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "v_mul_u32_u24",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "v_mul_hi_u32_u24",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "v_dot4c_i32_i8",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "v_min_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x10,
        OpInfo {
            name: "v_max_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11,
        OpInfo {
            name: "v_min_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x12,
        OpInfo {
            name: "v_max_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x13,
        OpInfo {
            name: "v_min_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x14,
        OpInfo {
            name: "v_max_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x16,
        OpInfo {
            name: "v_lshrrev_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "v_ashrrev_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "v_lshlrev_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "v_and_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c,
        OpInfo {
            name: "v_or_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d,
        OpInfo {
            name: "v_xor_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1e,
        OpInfo {
            name: "v_xnor_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "v_mac_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "v_madmk_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Simm32,
                    regs: 0,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "v_madak_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
                Spec {
                    field: Simm32,
                    regs: 0,
                },
            ],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "v_add_nc_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x26,
        OpInfo {
            name: "v_sub_nc_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x27,
        OpInfo {
            name: "v_subrev_nc_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x28,
        OpInfo {
            name: "v_add_co_ci_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
                Spec {
                    field: Vcc,
                    regs: -1,
                },
            ],
        },
    ),
    (
        0x29,
        OpInfo {
            name: "v_sub_co_ci_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
                Spec {
                    field: Vcc,
                    regs: -1,
                },
            ],
        },
    ),
    (
        0x2a,
        OpInfo {
            name: "v_subrev_co_ci_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
                Spec {
                    field: Vcc,
                    regs: -1,
                },
            ],
        },
    ),
    (
        0x2b,
        OpInfo {
            name: "v_fmac_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2c,
        OpInfo {
            name: "v_fmamk_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Literal,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2d,
        OpInfo {
            name: "v_fmaak_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
                Spec {
                    field: Literal,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2f,
        OpInfo {
            name: "v_cvt_pkrtz_f16_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x32,
        OpInfo {
            name: "v_add_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x33,
        OpInfo {
            name: "v_sub_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x34,
        OpInfo {
            name: "v_subrev_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x35,
        OpInfo {
            name: "v_mul_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x36,
        OpInfo {
            name: "v_fmac_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x37,
        OpInfo {
            name: "v_fmamk_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Literal,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x38,
        OpInfo {
            name: "v_fmaak_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
                Spec {
                    field: Literal,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x39,
        OpInfo {
            name: "v_max_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3a,
        OpInfo {
            name: "v_min_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3b,
        OpInfo {
            name: "v_ldexp_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3c,
        OpInfo {
            name: "v_pk_fmac_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static VOP3: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "v_cmp_f_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "v_cmp_lt_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "v_cmp_eq_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "v_cmp_le_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "v_cmp_gt_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "v_cmp_lg_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "v_cmp_ge_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "v_cmp_o_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "v_cmp_u_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "v_cmp_nge_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "v_cmp_nlg_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "v_cmp_ngt_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "v_cmp_nle_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "v_cmp_neq_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "v_cmp_nlt_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "v_cmp_tru_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x10,
        OpInfo {
            name: "v_cmpx_f_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11,
        OpInfo {
            name: "v_cmpx_lt_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x12,
        OpInfo {
            name: "v_cmpx_eq_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x13,
        OpInfo {
            name: "v_cmpx_le_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x14,
        OpInfo {
            name: "v_cmpx_gt_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x15,
        OpInfo {
            name: "v_cmpx_lg_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x16,
        OpInfo {
            name: "v_cmpx_ge_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x17,
        OpInfo {
            name: "v_cmpx_o_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "v_cmpx_u_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "v_cmpx_nge_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "v_cmpx_nlg_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "v_cmpx_ngt_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c,
        OpInfo {
            name: "v_cmpx_nle_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d,
        OpInfo {
            name: "v_cmpx_neq_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1e,
        OpInfo {
            name: "v_cmpx_nlt_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "v_cmpx_tru_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "v_cmp_f_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "v_cmp_lt_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x22,
        OpInfo {
            name: "v_cmp_eq_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x23,
        OpInfo {
            name: "v_cmp_le_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "v_cmp_gt_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "v_cmp_lg_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x26,
        OpInfo {
            name: "v_cmp_ge_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x27,
        OpInfo {
            name: "v_cmp_o_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x28,
        OpInfo {
            name: "v_cmp_u_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x29,
        OpInfo {
            name: "v_cmp_nge_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2a,
        OpInfo {
            name: "v_cmp_nlg_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2b,
        OpInfo {
            name: "v_cmp_ngt_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2c,
        OpInfo {
            name: "v_cmp_nle_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2d,
        OpInfo {
            name: "v_cmp_neq_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2e,
        OpInfo {
            name: "v_cmp_nlt_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2f,
        OpInfo {
            name: "v_cmp_tru_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x30,
        OpInfo {
            name: "v_cmpx_f_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x31,
        OpInfo {
            name: "v_cmpx_lt_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x32,
        OpInfo {
            name: "v_cmpx_eq_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x33,
        OpInfo {
            name: "v_cmpx_le_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x34,
        OpInfo {
            name: "v_cmpx_gt_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x35,
        OpInfo {
            name: "v_cmpx_lg_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x36,
        OpInfo {
            name: "v_cmpx_ge_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x37,
        OpInfo {
            name: "v_cmpx_o_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x38,
        OpInfo {
            name: "v_cmpx_u_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x39,
        OpInfo {
            name: "v_cmpx_nge_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3a,
        OpInfo {
            name: "v_cmpx_nlg_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3b,
        OpInfo {
            name: "v_cmpx_ngt_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3c,
        OpInfo {
            name: "v_cmpx_nle_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3d,
        OpInfo {
            name: "v_cmpx_neq_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3e,
        OpInfo {
            name: "v_cmpx_nlt_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3f,
        OpInfo {
            name: "v_cmpx_tru_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x80,
        OpInfo {
            name: "v_cmp_f_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x81,
        OpInfo {
            name: "v_cmp_lt_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x82,
        OpInfo {
            name: "v_cmp_eq_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x83,
        OpInfo {
            name: "v_cmp_le_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x84,
        OpInfo {
            name: "v_cmp_gt_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x85,
        OpInfo {
            name: "v_cmp_ne_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x86,
        OpInfo {
            name: "v_cmp_ge_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x87,
        OpInfo {
            name: "v_cmp_t_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x88,
        OpInfo {
            name: "v_cmp_class_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x89,
        OpInfo {
            name: "v_cmp_lt_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8a,
        OpInfo {
            name: "v_cmp_eq_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8b,
        OpInfo {
            name: "v_cmp_le_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8c,
        OpInfo {
            name: "v_cmp_gt_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8d,
        OpInfo {
            name: "v_cmp_ne_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8e,
        OpInfo {
            name: "v_cmp_ge_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8f,
        OpInfo {
            name: "v_cmp_class_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x90,
        OpInfo {
            name: "v_cmpx_f_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x91,
        OpInfo {
            name: "v_cmpx_lt_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x92,
        OpInfo {
            name: "v_cmpx_eq_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x93,
        OpInfo {
            name: "v_cmpx_le_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x94,
        OpInfo {
            name: "v_cmpx_gt_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x95,
        OpInfo {
            name: "v_cmpx_ne_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x96,
        OpInfo {
            name: "v_cmpx_ge_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x97,
        OpInfo {
            name: "v_cmpx_t_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x98,
        OpInfo {
            name: "v_cmpx_class_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x99,
        OpInfo {
            name: "v_cmpx_lt_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9a,
        OpInfo {
            name: "v_cmpx_eq_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9b,
        OpInfo {
            name: "v_cmpx_le_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9c,
        OpInfo {
            name: "v_cmpx_gt_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9d,
        OpInfo {
            name: "v_cmpx_ne_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9e,
        OpInfo {
            name: "v_cmpx_ge_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9f,
        OpInfo {
            name: "v_cmpx_class_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa0,
        OpInfo {
            name: "v_cmp_f_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa1,
        OpInfo {
            name: "v_cmp_lt_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa2,
        OpInfo {
            name: "v_cmp_eq_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa3,
        OpInfo {
            name: "v_cmp_le_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa4,
        OpInfo {
            name: "v_cmp_gt_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa5,
        OpInfo {
            name: "v_cmp_ne_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa6,
        OpInfo {
            name: "v_cmp_ge_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa7,
        OpInfo {
            name: "v_cmp_t_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa8,
        OpInfo {
            name: "v_cmp_class_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa9,
        OpInfo {
            name: "v_cmp_lt_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xaa,
        OpInfo {
            name: "v_cmp_eq_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xab,
        OpInfo {
            name: "v_cmp_le_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xac,
        OpInfo {
            name: "v_cmp_gt_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xad,
        OpInfo {
            name: "v_cmp_ne_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xae,
        OpInfo {
            name: "v_cmp_ge_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb0,
        OpInfo {
            name: "v_cmpx_f_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb1,
        OpInfo {
            name: "v_cmpx_lt_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb2,
        OpInfo {
            name: "v_cmpx_eq_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb3,
        OpInfo {
            name: "v_cmpx_le_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb4,
        OpInfo {
            name: "v_cmpx_gt_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb5,
        OpInfo {
            name: "v_cmpx_ne_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb6,
        OpInfo {
            name: "v_cmpx_ge_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb7,
        OpInfo {
            name: "v_cmpx_t_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb8,
        OpInfo {
            name: "v_cmpx_class_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb9,
        OpInfo {
            name: "v_cmpx_lt_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xba,
        OpInfo {
            name: "v_cmpx_eq_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xbb,
        OpInfo {
            name: "v_cmpx_le_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xbc,
        OpInfo {
            name: "v_cmpx_gt_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xbd,
        OpInfo {
            name: "v_cmpx_ne_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xbe,
        OpInfo {
            name: "v_cmpx_ge_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc0,
        OpInfo {
            name: "v_cmp_f_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc1,
        OpInfo {
            name: "v_cmp_lt_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc2,
        OpInfo {
            name: "v_cmp_eq_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc3,
        OpInfo {
            name: "v_cmp_le_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc4,
        OpInfo {
            name: "v_cmp_gt_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc5,
        OpInfo {
            name: "v_cmp_ne_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc6,
        OpInfo {
            name: "v_cmp_ge_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc7,
        OpInfo {
            name: "v_cmp_t_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc8,
        OpInfo {
            name: "v_cmp_f_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc9,
        OpInfo {
            name: "v_cmp_lt_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xca,
        OpInfo {
            name: "v_cmp_eq_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xcb,
        OpInfo {
            name: "v_cmp_le_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xcc,
        OpInfo {
            name: "v_cmp_gt_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xcd,
        OpInfo {
            name: "v_cmp_lg_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xce,
        OpInfo {
            name: "v_cmp_ge_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xcf,
        OpInfo {
            name: "v_cmp_o_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd0,
        OpInfo {
            name: "v_cmpx_f_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd1,
        OpInfo {
            name: "v_cmpx_lt_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd2,
        OpInfo {
            name: "v_cmpx_eq_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd3,
        OpInfo {
            name: "v_cmpx_le_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd4,
        OpInfo {
            name: "v_cmpx_gt_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd5,
        OpInfo {
            name: "v_cmpx_ne_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd6,
        OpInfo {
            name: "v_cmpx_ge_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd7,
        OpInfo {
            name: "v_cmpx_t_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd8,
        OpInfo {
            name: "v_cmpx_f_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd9,
        OpInfo {
            name: "v_cmpx_lt_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xda,
        OpInfo {
            name: "v_cmpx_eq_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xdb,
        OpInfo {
            name: "v_cmpx_le_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xdc,
        OpInfo {
            name: "v_cmpx_gt_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xdd,
        OpInfo {
            name: "v_cmpx_lg_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xde,
        OpInfo {
            name: "v_cmpx_ge_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xdf,
        OpInfo {
            name: "v_cmpx_o_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe0,
        OpInfo {
            name: "v_cmp_f_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe1,
        OpInfo {
            name: "v_cmp_lt_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe2,
        OpInfo {
            name: "v_cmp_eq_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe3,
        OpInfo {
            name: "v_cmp_le_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe4,
        OpInfo {
            name: "v_cmp_gt_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe5,
        OpInfo {
            name: "v_cmp_ne_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe6,
        OpInfo {
            name: "v_cmp_ge_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe7,
        OpInfo {
            name: "v_cmp_t_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe8,
        OpInfo {
            name: "v_cmp_u_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe9,
        OpInfo {
            name: "v_cmp_nge_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xea,
        OpInfo {
            name: "v_cmp_nlg_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xeb,
        OpInfo {
            name: "v_cmp_ngt_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xec,
        OpInfo {
            name: "v_cmp_nle_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xed,
        OpInfo {
            name: "v_cmp_neq_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xee,
        OpInfo {
            name: "v_cmp_nlt_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xef,
        OpInfo {
            name: "v_cmp_tru_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf0,
        OpInfo {
            name: "v_cmpx_f_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf1,
        OpInfo {
            name: "v_cmpx_lt_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf2,
        OpInfo {
            name: "v_cmpx_eq_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf3,
        OpInfo {
            name: "v_cmpx_le_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf4,
        OpInfo {
            name: "v_cmpx_gt_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf5,
        OpInfo {
            name: "v_cmpx_ne_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf6,
        OpInfo {
            name: "v_cmpx_ge_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf7,
        OpInfo {
            name: "v_cmpx_t_u64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf8,
        OpInfo {
            name: "v_cmpx_u_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf9,
        OpInfo {
            name: "v_cmpx_nge_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xfa,
        OpInfo {
            name: "v_cmpx_nlg_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xfb,
        OpInfo {
            name: "v_cmpx_ngt_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xfc,
        OpInfo {
            name: "v_cmpx_nle_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xfd,
        OpInfo {
            name: "v_cmpx_neq_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xfe,
        OpInfo {
            name: "v_cmpx_nlt_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xff,
        OpInfo {
            name: "v_cmpx_tru_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x101,
        OpInfo {
            name: "v_cndmask_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: -1,
                },
            ],
        },
    ),
    (
        0x103,
        OpInfo {
            name: "v_add_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x104,
        OpInfo {
            name: "v_sub_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x105,
        OpInfo {
            name: "v_subrev_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x106,
        OpInfo {
            name: "v_fmac_legacy_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x107,
        OpInfo {
            name: "v_mul_legacy_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x108,
        OpInfo {
            name: "v_mul_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x109,
        OpInfo {
            name: "v_mul_i32_i24",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x10a,
        OpInfo {
            name: "v_mul_hi_i32_i24",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x10b,
        OpInfo {
            name: "v_mul_u32_u24",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x10c,
        OpInfo {
            name: "v_mul_hi_u32_u24",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x10f,
        OpInfo {
            name: "v_min_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x110,
        OpInfo {
            name: "v_max_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x111,
        OpInfo {
            name: "v_min_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x112,
        OpInfo {
            name: "v_max_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x113,
        OpInfo {
            name: "v_min_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x114,
        OpInfo {
            name: "v_max_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x116,
        OpInfo {
            name: "v_lshrrev_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x118,
        OpInfo {
            name: "v_ashrrev_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11a,
        OpInfo {
            name: "v_lshlrev_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11b,
        OpInfo {
            name: "v_and_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11c,
        OpInfo {
            name: "v_or_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11d,
        OpInfo {
            name: "v_xor_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11e,
        OpInfo {
            name: "v_xnor_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11f,
        OpInfo {
            name: "v_mac_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x125,
        OpInfo {
            name: "v_add_nc_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x126,
        OpInfo {
            name: "v_sub_nc_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x127,
        OpInfo {
            name: "v_subrev_nc_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x12b,
        OpInfo {
            name: "v_fmac_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x12f,
        OpInfo {
            name: "v_cvt_pkrtz_f16_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x132,
        OpInfo {
            name: "v_add_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x133,
        OpInfo {
            name: "v_sub_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x134,
        OpInfo {
            name: "v_subrev_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x135,
        OpInfo {
            name: "v_mul_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x136,
        OpInfo {
            name: "v_fmac_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x139,
        OpInfo {
            name: "v_max_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x13a,
        OpInfo {
            name: "v_min_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x13b,
        OpInfo {
            name: "v_ldexp_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x140,
        OpInfo {
            name: "v_fma_legacy_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x141,
        OpInfo {
            name: "v_mad_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x142,
        OpInfo {
            name: "v_mad_i32_i24",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x143,
        OpInfo {
            name: "v_mad_u32_u24",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x144,
        OpInfo {
            name: "v_cubeid_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x145,
        OpInfo {
            name: "v_cubesc_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x146,
        OpInfo {
            name: "v_cubetc_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x147,
        OpInfo {
            name: "v_cubema_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x148,
        OpInfo {
            name: "v_bfe_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x149,
        OpInfo {
            name: "v_bfe_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x14a,
        OpInfo {
            name: "v_bfi_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x14b,
        OpInfo {
            name: "v_fma_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x14c,
        OpInfo {
            name: "v_fma_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
                Spec {
                    field: Src2,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x14d,
        OpInfo {
            name: "v_lerp_u8",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x14e,
        OpInfo {
            name: "v_alignbit_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x14f,
        OpInfo {
            name: "v_alignbyte_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x150,
        OpInfo {
            name: "v_mullit_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x151,
        OpInfo {
            name: "v_min3_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x152,
        OpInfo {
            name: "v_min3_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x153,
        OpInfo {
            name: "v_min3_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x154,
        OpInfo {
            name: "v_max3_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x155,
        OpInfo {
            name: "v_max3_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x156,
        OpInfo {
            name: "v_max3_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x157,
        OpInfo {
            name: "v_med3_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x158,
        OpInfo {
            name: "v_med3_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x159,
        OpInfo {
            name: "v_med3_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x15a,
        OpInfo {
            name: "v_sad_u8",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x15b,
        OpInfo {
            name: "v_sad_hi_u8",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x15c,
        OpInfo {
            name: "v_sad_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x15d,
        OpInfo {
            name: "v_sad_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x15e,
        OpInfo {
            name: "v_cvt_pk_u8_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x15f,
        OpInfo {
            name: "v_div_fixup_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x160,
        OpInfo {
            name: "v_div_fixup_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
                Spec {
                    field: Src2,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x164,
        OpInfo {
            name: "v_add_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x165,
        OpInfo {
            name: "v_mul_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x166,
        OpInfo {
            name: "v_min_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x167,
        OpInfo {
            name: "v_max_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x168,
        OpInfo {
            name: "v_ldexp_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x169,
        OpInfo {
            name: "v_mul_lo_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x16a,
        OpInfo {
            name: "v_mul_hi_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x16c,
        OpInfo {
            name: "v_mul_hi_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x16f,
        OpInfo {
            name: "v_div_fmas_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x170,
        OpInfo {
            name: "v_div_fmas_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
                Spec {
                    field: Src2,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x171,
        OpInfo {
            name: "v_msad_u8",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x172,
        OpInfo {
            name: "v_qsad_pk_u16_u8",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x173,
        OpInfo {
            name: "v_mqsad_pk_u16_u8",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x174,
        OpInfo {
            name: "v_trig_preop_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x175,
        OpInfo {
            name: "v_mqsad_u32_u8",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 4,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 4,
                },
            ],
        },
    ),
    (
        0x178,
        OpInfo {
            name: "v_xor3_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x180,
        OpInfo {
            name: "v_nop",
            spec: &[],
        },
    ),
    (
        0x181,
        OpInfo {
            name: "v_mov_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x182,
        OpInfo {
            name: "v_readfirstlane_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x183,
        OpInfo {
            name: "v_cvt_i32_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x184,
        OpInfo {
            name: "v_cvt_f64_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x185,
        OpInfo {
            name: "v_cvt_f32_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x186,
        OpInfo {
            name: "v_cvt_f32_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x187,
        OpInfo {
            name: "v_cvt_u32_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x188,
        OpInfo {
            name: "v_cvt_i32_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18a,
        OpInfo {
            name: "v_cvt_f16_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18b,
        OpInfo {
            name: "v_cvt_f32_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18c,
        OpInfo {
            name: "v_cvt_rpi_i32_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18d,
        OpInfo {
            name: "v_cvt_flr_i32_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18e,
        OpInfo {
            name: "v_cvt_off_f32_i4",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18f,
        OpInfo {
            name: "v_cvt_f32_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x190,
        OpInfo {
            name: "v_cvt_f64_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x191,
        OpInfo {
            name: "v_cvt_f32_ubyte0",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x192,
        OpInfo {
            name: "v_cvt_f32_ubyte1",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x193,
        OpInfo {
            name: "v_cvt_f32_ubyte2",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x194,
        OpInfo {
            name: "v_cvt_f32_ubyte3",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x195,
        OpInfo {
            name: "v_cvt_u32_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x196,
        OpInfo {
            name: "v_cvt_f64_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x197,
        OpInfo {
            name: "v_trunc_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x198,
        OpInfo {
            name: "v_ceil_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x199,
        OpInfo {
            name: "v_rndne_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x19a,
        OpInfo {
            name: "v_floor_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x19b,
        OpInfo {
            name: "v_pipeflush",
            spec: &[],
        },
    ),
    (
        0x1a0,
        OpInfo {
            name: "v_fract_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a1,
        OpInfo {
            name: "v_trunc_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a2,
        OpInfo {
            name: "v_ceil_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a3,
        OpInfo {
            name: "v_rndne_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a4,
        OpInfo {
            name: "v_floor_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a5,
        OpInfo {
            name: "v_exp_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a7,
        OpInfo {
            name: "v_log_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1aa,
        OpInfo {
            name: "v_rcp_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1ab,
        OpInfo {
            name: "v_rcp_iflag_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1ae,
        OpInfo {
            name: "v_rsq_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1af,
        OpInfo {
            name: "v_rcp_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1b1,
        OpInfo {
            name: "v_rsq_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1b3,
        OpInfo {
            name: "v_sqrt_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b4,
        OpInfo {
            name: "v_sqrt_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1b5,
        OpInfo {
            name: "v_sin_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b6,
        OpInfo {
            name: "v_cos_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b7,
        OpInfo {
            name: "v_not_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b8,
        OpInfo {
            name: "v_bfrev_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b9,
        OpInfo {
            name: "v_ffbh_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1ba,
        OpInfo {
            name: "v_ffbl_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1bb,
        OpInfo {
            name: "v_ffbh_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1bc,
        OpInfo {
            name: "v_frexp_exp_i32_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1bd,
        OpInfo {
            name: "v_frexp_mant_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1be,
        OpInfo {
            name: "v_fract_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x1bf,
        OpInfo {
            name: "v_frexp_exp_i32_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c0,
        OpInfo {
            name: "v_frexp_mant_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c1,
        OpInfo {
            name: "v_clrexcp",
            spec: &[],
        },
    ),
    (
        0x1c2,
        OpInfo {
            name: "v_movreld_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c3,
        OpInfo {
            name: "v_movrels_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c4,
        OpInfo {
            name: "v_movrelsd_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c8,
        OpInfo {
            name: "v_movrelsd_2_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d0,
        OpInfo {
            name: "v_cvt_f16_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d1,
        OpInfo {
            name: "v_cvt_f16_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d2,
        OpInfo {
            name: "v_cvt_u16_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d3,
        OpInfo {
            name: "v_cvt_i16_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d4,
        OpInfo {
            name: "v_rcp_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d5,
        OpInfo {
            name: "v_sqrt_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d6,
        OpInfo {
            name: "v_rsq_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d7,
        OpInfo {
            name: "v_log_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d8,
        OpInfo {
            name: "v_exp_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d9,
        OpInfo {
            name: "v_frexp_mant_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1da,
        OpInfo {
            name: "v_frexp_exp_i16_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1db,
        OpInfo {
            name: "v_floor_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1dc,
        OpInfo {
            name: "v_ceil_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1dd,
        OpInfo {
            name: "v_trunc_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1de,
        OpInfo {
            name: "v_rndne_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1df,
        OpInfo {
            name: "v_fract_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1e0,
        OpInfo {
            name: "v_sin_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1e1,
        OpInfo {
            name: "v_cos_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1e2,
        OpInfo {
            name: "v_sat_pk_u8_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1e3,
        OpInfo {
            name: "v_cvt_norm_i16_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1e4,
        OpInfo {
            name: "v_cvt_norm_u16_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x200,
        OpInfo {
            name: "v_interp_p1_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x201,
        OpInfo {
            name: "v_interp_p2_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x202,
        OpInfo {
            name: "v_interp_mov_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2ff,
        OpInfo {
            name: "v_lshlrev_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x300,
        OpInfo {
            name: "v_lshrrev_b64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x301,
        OpInfo {
            name: "v_ashrrev_i64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x303,
        OpInfo {
            name: "v_add_nc_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x304,
        OpInfo {
            name: "v_sub_nc_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x305,
        OpInfo {
            name: "v_mul_lo_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x307,
        OpInfo {
            name: "v_lshrrev_b16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x308,
        OpInfo {
            name: "v_ashrrev_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x309,
        OpInfo {
            name: "v_max_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x30a,
        OpInfo {
            name: "v_max_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x30b,
        OpInfo {
            name: "v_min_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x30c,
        OpInfo {
            name: "v_min_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x30d,
        OpInfo {
            name: "v_add_nc_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x30e,
        OpInfo {
            name: "v_sub_nc_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x311,
        OpInfo {
            name: "v_pack_b32_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x312,
        OpInfo {
            name: "v_cvt_pknorm_i16_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x313,
        OpInfo {
            name: "v_cvt_pknorm_u16_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x314,
        OpInfo {
            name: "v_lshlrev_b16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x340,
        OpInfo {
            name: "v_mad_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x342,
        OpInfo {
            name: "v_interp_p1ll_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x343,
        OpInfo {
            name: "v_interp_p1lv_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x344,
        OpInfo {
            name: "v_perm_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x345,
        OpInfo {
            name: "v_xad_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x346,
        OpInfo {
            name: "v_lshl_add_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x347,
        OpInfo {
            name: "v_add_lshl_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x34b,
        OpInfo {
            name: "v_fma_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x351,
        OpInfo {
            name: "v_min3_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x352,
        OpInfo {
            name: "v_min3_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x353,
        OpInfo {
            name: "v_min3_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x354,
        OpInfo {
            name: "v_max3_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x355,
        OpInfo {
            name: "v_max3_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x356,
        OpInfo {
            name: "v_max3_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x357,
        OpInfo {
            name: "v_med3_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x358,
        OpInfo {
            name: "v_med3_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x359,
        OpInfo {
            name: "v_med3_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x35a,
        OpInfo {
            name: "v_interp_p2_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x35e,
        OpInfo {
            name: "v_mad_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x35f,
        OpInfo {
            name: "v_div_fixup_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x360,
        OpInfo {
            name: "v_readlane_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x361,
        OpInfo {
            name: "v_writelane_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x362,
        OpInfo {
            name: "v_ldexp_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x363,
        OpInfo {
            name: "v_bfm_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x364,
        OpInfo {
            name: "v_bcnt_u32_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x365,
        OpInfo {
            name: "v_mbcnt_lo_u32_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x366,
        OpInfo {
            name: "v_mbcnt_hi_u32_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x368,
        OpInfo {
            name: "v_cvt_pknorm_i16_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x369,
        OpInfo {
            name: "v_cvt_pknorm_u16_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x36a,
        OpInfo {
            name: "v_cvt_pk_u16_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x36b,
        OpInfo {
            name: "v_cvt_pk_i16_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x36d,
        OpInfo {
            name: "v_add3_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x36f,
        OpInfo {
            name: "v_lshl_or_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x371,
        OpInfo {
            name: "v_and_or_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x372,
        OpInfo {
            name: "v_or3_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x373,
        OpInfo {
            name: "v_mad_u32_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x375,
        OpInfo {
            name: "v_mad_i32_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x376,
        OpInfo {
            name: "v_sub_nc_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x377,
        OpInfo {
            name: "v_permlane16_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x378,
        OpInfo {
            name: "v_permlanex16_b32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x37f,
        OpInfo {
            name: "v_add_nc_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static VOP3B: &[(u16, OpInfo)] = &[
    (
        0x128,
        OpInfo {
            name: "v_add_co_ci_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Sdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: -1,
                },
            ],
        },
    ),
    (
        0x129,
        OpInfo {
            name: "v_sub_co_ci_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Sdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: -1,
                },
            ],
        },
    ),
    (
        0x12a,
        OpInfo {
            name: "v_subrev_co_ci_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Sdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: -1,
                },
            ],
        },
    ),
    (
        0x16d,
        OpInfo {
            name: "v_div_scale_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Sdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x16e,
        OpInfo {
            name: "v_div_scale_f64",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Sdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Src1,
                    regs: 2,
                },
                Spec {
                    field: Src2,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x176,
        OpInfo {
            name: "v_mad_u64_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Sdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x177,
        OpInfo {
            name: "v_mad_i64_i32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 2,
                },
                Spec {
                    field: Sdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x30f,
        OpInfo {
            name: "v_add_co_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Sdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x310,
        OpInfo {
            name: "v_sub_co_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Sdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x319,
        OpInfo {
            name: "v_subrev_co_u32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Sdst,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static VOP3P: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "v_pk_mad_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "v_pk_mul_lo_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "v_pk_add_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "v_pk_sub_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "v_pk_lshlrev_b16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "v_pk_lshrrev_b16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "v_pk_ashrrev_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "v_pk_max_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "v_pk_min_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "v_pk_mad_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "v_pk_add_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "v_pk_sub_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "v_pk_max_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "v_pk_min_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "v_pk_fma_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "v_pk_add_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x10,
        OpInfo {
            name: "v_pk_mul_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11,
        OpInfo {
            name: "v_pk_min_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x12,
        OpInfo {
            name: "v_pk_max_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x13,
        OpInfo {
            name: "v_dot2_f32_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x14,
        OpInfo {
            name: "v_dot2_i32_i16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x15,
        OpInfo {
            name: "v_dot2_u32_u16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x16,
        OpInfo {
            name: "v_dot4_i32_i8",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x17,
        OpInfo {
            name: "v_dot4_u32_u8",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "v_dot8_i32_i4",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "v_dot8_u32_u4",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "v_fma_mix_f32",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "v_fma_mixlo_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x22,
        OpInfo {
            name: "v_fma_mixhi_f16",
            spec: &[
                Spec {
                    field: Vdst,
                    regs: 1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Src1,
                    regs: 1,
                },
                Spec {
                    field: Src2,
                    regs: 1,
                },
            ],
        },
    ),
];

pub static VOPC: &[(u16, OpInfo)] = &[
    (
        0x0,
        OpInfo {
            name: "v_cmp_f_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1,
        OpInfo {
            name: "v_cmp_lt_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x2,
        OpInfo {
            name: "v_cmp_eq_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x3,
        OpInfo {
            name: "v_cmp_le_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x4,
        OpInfo {
            name: "v_cmp_gt_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x5,
        OpInfo {
            name: "v_cmp_lg_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x6,
        OpInfo {
            name: "v_cmp_ge_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x7,
        OpInfo {
            name: "v_cmp_o_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8,
        OpInfo {
            name: "v_cmp_u_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9,
        OpInfo {
            name: "v_cmp_nge_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa,
        OpInfo {
            name: "v_cmp_nlg_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb,
        OpInfo {
            name: "v_cmp_ngt_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc,
        OpInfo {
            name: "v_cmp_nle_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd,
        OpInfo {
            name: "v_cmp_neq_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe,
        OpInfo {
            name: "v_cmp_nlt_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf,
        OpInfo {
            name: "v_cmp_tru_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x10,
        OpInfo {
            name: "v_cmpx_f_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x11,
        OpInfo {
            name: "v_cmpx_lt_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x12,
        OpInfo {
            name: "v_cmpx_eq_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x13,
        OpInfo {
            name: "v_cmpx_le_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x14,
        OpInfo {
            name: "v_cmpx_gt_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x15,
        OpInfo {
            name: "v_cmpx_lg_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x16,
        OpInfo {
            name: "v_cmpx_ge_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x17,
        OpInfo {
            name: "v_cmpx_o_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x18,
        OpInfo {
            name: "v_cmpx_u_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x19,
        OpInfo {
            name: "v_cmpx_nge_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1a,
        OpInfo {
            name: "v_cmpx_nlg_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1b,
        OpInfo {
            name: "v_cmpx_ngt_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1c,
        OpInfo {
            name: "v_cmpx_nle_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1d,
        OpInfo {
            name: "v_cmpx_neq_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1e,
        OpInfo {
            name: "v_cmpx_nlt_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x1f,
        OpInfo {
            name: "v_cmpx_tru_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x20,
        OpInfo {
            name: "v_cmp_f_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x21,
        OpInfo {
            name: "v_cmp_lt_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x22,
        OpInfo {
            name: "v_cmp_eq_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x23,
        OpInfo {
            name: "v_cmp_le_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x24,
        OpInfo {
            name: "v_cmp_gt_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x25,
        OpInfo {
            name: "v_cmp_lg_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x26,
        OpInfo {
            name: "v_cmp_ge_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x27,
        OpInfo {
            name: "v_cmp_o_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x28,
        OpInfo {
            name: "v_cmp_u_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x29,
        OpInfo {
            name: "v_cmp_nge_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2a,
        OpInfo {
            name: "v_cmp_nlg_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2b,
        OpInfo {
            name: "v_cmp_ngt_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2c,
        OpInfo {
            name: "v_cmp_nle_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2d,
        OpInfo {
            name: "v_cmp_neq_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2e,
        OpInfo {
            name: "v_cmp_nlt_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x2f,
        OpInfo {
            name: "v_cmp_tru_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x30,
        OpInfo {
            name: "v_cmpx_f_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x31,
        OpInfo {
            name: "v_cmpx_lt_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x32,
        OpInfo {
            name: "v_cmpx_eq_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x33,
        OpInfo {
            name: "v_cmpx_le_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x34,
        OpInfo {
            name: "v_cmpx_gt_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x35,
        OpInfo {
            name: "v_cmpx_lg_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x36,
        OpInfo {
            name: "v_cmpx_ge_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x37,
        OpInfo {
            name: "v_cmpx_o_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x38,
        OpInfo {
            name: "v_cmpx_u_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x39,
        OpInfo {
            name: "v_cmpx_nge_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3a,
        OpInfo {
            name: "v_cmpx_nlg_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3b,
        OpInfo {
            name: "v_cmpx_ngt_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3c,
        OpInfo {
            name: "v_cmpx_nle_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3d,
        OpInfo {
            name: "v_cmpx_neq_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3e,
        OpInfo {
            name: "v_cmpx_nlt_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x3f,
        OpInfo {
            name: "v_cmpx_tru_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0x80,
        OpInfo {
            name: "v_cmp_f_i32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x81,
        OpInfo {
            name: "v_cmp_lt_i32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x82,
        OpInfo {
            name: "v_cmp_eq_i32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x83,
        OpInfo {
            name: "v_cmp_le_i32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x84,
        OpInfo {
            name: "v_cmp_gt_i32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x85,
        OpInfo {
            name: "v_cmp_ne_i32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x86,
        OpInfo {
            name: "v_cmp_ge_i32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x87,
        OpInfo {
            name: "v_cmp_t_i32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x88,
        OpInfo {
            name: "v_cmp_class_f32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x89,
        OpInfo {
            name: "v_cmp_lt_i16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8a,
        OpInfo {
            name: "v_cmp_eq_i16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8b,
        OpInfo {
            name: "v_cmp_le_i16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8c,
        OpInfo {
            name: "v_cmp_gt_i16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8d,
        OpInfo {
            name: "v_cmp_ne_i16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8e,
        OpInfo {
            name: "v_cmp_ge_i16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x8f,
        OpInfo {
            name: "v_cmp_class_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x90,
        OpInfo {
            name: "v_cmpx_f_i32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x91,
        OpInfo {
            name: "v_cmpx_lt_i32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x92,
        OpInfo {
            name: "v_cmpx_eq_i32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x93,
        OpInfo {
            name: "v_cmpx_le_i32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x94,
        OpInfo {
            name: "v_cmpx_gt_i32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x95,
        OpInfo {
            name: "v_cmpx_ne_i32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x96,
        OpInfo {
            name: "v_cmpx_ge_i32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x97,
        OpInfo {
            name: "v_cmpx_t_i32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x98,
        OpInfo {
            name: "v_cmpx_class_f32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x99,
        OpInfo {
            name: "v_cmpx_lt_i16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9a,
        OpInfo {
            name: "v_cmpx_eq_i16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9b,
        OpInfo {
            name: "v_cmpx_le_i16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9c,
        OpInfo {
            name: "v_cmpx_gt_i16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9d,
        OpInfo {
            name: "v_cmpx_ne_i16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9e,
        OpInfo {
            name: "v_cmpx_ge_i16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0x9f,
        OpInfo {
            name: "v_cmpx_class_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa0,
        OpInfo {
            name: "v_cmp_f_i64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa1,
        OpInfo {
            name: "v_cmp_lt_i64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa2,
        OpInfo {
            name: "v_cmp_eq_i64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa3,
        OpInfo {
            name: "v_cmp_le_i64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa4,
        OpInfo {
            name: "v_cmp_gt_i64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa5,
        OpInfo {
            name: "v_cmp_ne_i64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa6,
        OpInfo {
            name: "v_cmp_ge_i64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa7,
        OpInfo {
            name: "v_cmp_t_i64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xa8,
        OpInfo {
            name: "v_cmp_class_f64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xa9,
        OpInfo {
            name: "v_cmp_lt_u16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xaa,
        OpInfo {
            name: "v_cmp_eq_u16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xab,
        OpInfo {
            name: "v_cmp_le_u16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xac,
        OpInfo {
            name: "v_cmp_gt_u16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xad,
        OpInfo {
            name: "v_cmp_ne_u16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xae,
        OpInfo {
            name: "v_cmp_ge_u16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb0,
        OpInfo {
            name: "v_cmpx_f_i64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb1,
        OpInfo {
            name: "v_cmpx_lt_i64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb2,
        OpInfo {
            name: "v_cmpx_eq_i64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb3,
        OpInfo {
            name: "v_cmpx_le_i64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb4,
        OpInfo {
            name: "v_cmpx_gt_i64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb5,
        OpInfo {
            name: "v_cmpx_ne_i64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb6,
        OpInfo {
            name: "v_cmpx_ge_i64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb7,
        OpInfo {
            name: "v_cmpx_t_i64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xb8,
        OpInfo {
            name: "v_cmpx_class_f64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xb9,
        OpInfo {
            name: "v_cmpx_lt_u16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xba,
        OpInfo {
            name: "v_cmpx_eq_u16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xbb,
        OpInfo {
            name: "v_cmpx_le_u16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xbc,
        OpInfo {
            name: "v_cmpx_gt_u16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xbd,
        OpInfo {
            name: "v_cmpx_ne_u16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xbe,
        OpInfo {
            name: "v_cmpx_ge_u16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc0,
        OpInfo {
            name: "v_cmp_f_u32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc1,
        OpInfo {
            name: "v_cmp_lt_u32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc2,
        OpInfo {
            name: "v_cmp_eq_u32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc3,
        OpInfo {
            name: "v_cmp_le_u32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc4,
        OpInfo {
            name: "v_cmp_gt_u32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc5,
        OpInfo {
            name: "v_cmp_ne_u32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc6,
        OpInfo {
            name: "v_cmp_ge_u32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc7,
        OpInfo {
            name: "v_cmp_t_u32",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc8,
        OpInfo {
            name: "v_cmp_f_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xc9,
        OpInfo {
            name: "v_cmp_lt_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xca,
        OpInfo {
            name: "v_cmp_eq_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xcb,
        OpInfo {
            name: "v_cmp_le_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xcc,
        OpInfo {
            name: "v_cmp_gt_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xcd,
        OpInfo {
            name: "v_cmp_lg_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xce,
        OpInfo {
            name: "v_cmp_ge_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xcf,
        OpInfo {
            name: "v_cmp_o_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd0,
        OpInfo {
            name: "v_cmpx_f_u32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd1,
        OpInfo {
            name: "v_cmpx_lt_u32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd2,
        OpInfo {
            name: "v_cmpx_eq_u32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd3,
        OpInfo {
            name: "v_cmpx_le_u32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd4,
        OpInfo {
            name: "v_cmpx_gt_u32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd5,
        OpInfo {
            name: "v_cmpx_ne_u32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd6,
        OpInfo {
            name: "v_cmpx_ge_u32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd7,
        OpInfo {
            name: "v_cmpx_t_u32",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd8,
        OpInfo {
            name: "v_cmpx_f_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xd9,
        OpInfo {
            name: "v_cmpx_lt_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xda,
        OpInfo {
            name: "v_cmpx_eq_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xdb,
        OpInfo {
            name: "v_cmpx_le_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xdc,
        OpInfo {
            name: "v_cmpx_gt_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xdd,
        OpInfo {
            name: "v_cmpx_lg_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xde,
        OpInfo {
            name: "v_cmpx_ge_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xdf,
        OpInfo {
            name: "v_cmpx_o_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe0,
        OpInfo {
            name: "v_cmp_f_u64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe1,
        OpInfo {
            name: "v_cmp_lt_u64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe2,
        OpInfo {
            name: "v_cmp_eq_u64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe3,
        OpInfo {
            name: "v_cmp_le_u64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe4,
        OpInfo {
            name: "v_cmp_gt_u64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe5,
        OpInfo {
            name: "v_cmp_ne_u64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe6,
        OpInfo {
            name: "v_cmp_ge_u64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe7,
        OpInfo {
            name: "v_cmp_t_u64",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xe8,
        OpInfo {
            name: "v_cmp_u_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xe9,
        OpInfo {
            name: "v_cmp_nge_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xea,
        OpInfo {
            name: "v_cmp_nlg_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xeb,
        OpInfo {
            name: "v_cmp_ngt_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xec,
        OpInfo {
            name: "v_cmp_nle_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xed,
        OpInfo {
            name: "v_cmp_neq_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xee,
        OpInfo {
            name: "v_cmp_nlt_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xef,
        OpInfo {
            name: "v_cmp_tru_f16",
            spec: &[
                Spec {
                    field: Vcc,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf0,
        OpInfo {
            name: "v_cmpx_f_u64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf1,
        OpInfo {
            name: "v_cmpx_lt_u64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf2,
        OpInfo {
            name: "v_cmpx_eq_u64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf3,
        OpInfo {
            name: "v_cmpx_le_u64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf4,
        OpInfo {
            name: "v_cmpx_gt_u64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf5,
        OpInfo {
            name: "v_cmpx_ne_u64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf6,
        OpInfo {
            name: "v_cmpx_ge_u64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf7,
        OpInfo {
            name: "v_cmpx_t_u64",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 2,
                },
                Spec {
                    field: Vsrc1,
                    regs: 2,
                },
            ],
        },
    ),
    (
        0xf8,
        OpInfo {
            name: "v_cmpx_u_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xf9,
        OpInfo {
            name: "v_cmpx_nge_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xfa,
        OpInfo {
            name: "v_cmpx_nlg_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xfb,
        OpInfo {
            name: "v_cmpx_ngt_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xfc,
        OpInfo {
            name: "v_cmpx_nle_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xfd,
        OpInfo {
            name: "v_cmpx_neq_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xfe,
        OpInfo {
            name: "v_cmpx_nlt_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
    (
        0xff,
        OpInfo {
            name: "v_cmpx_tru_f16",
            spec: &[
                Spec {
                    field: OprExec,
                    regs: -1,
                },
                Spec {
                    field: Src0,
                    regs: 1,
                },
                Spec {
                    field: Vsrc1,
                    regs: 1,
                },
            ],
        },
    ),
];
