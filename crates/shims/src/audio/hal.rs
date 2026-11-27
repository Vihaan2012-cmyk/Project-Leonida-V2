//! The hardware abstraction object `AcpHalCreate` returns. The title calls it through a vtable of plain
//! functions: slot 0 starts it (`this`, 0x1000, 0x1000), slot 1 stops it, slot 2 submits a command (`this`,
//! operation, key, -1, payload address, 0) and slot 5 releases it. Only those four slots are called.
//!
//! The operations the title submits, all on one XMA context index:
//!
//! | operation | key | payload | meaning |
//! |---|---|---|---|
//! | 2 | 0xf0f0f0f0 | u32 0xc0 | configure at start-up |
//! | 5 | 0xf6f6f6f6 | context | enable the context |
//! | 7 | context + 1 | context | disable the context |
//! | 0x12 | 0xf1f1f1f1 | context, 256-byte blocks read | the title took that many output blocks |

/// Operations of slot 2.
pub const OP_CONFIGURE: u32 = 2;
pub const OP_ENABLE: u32 = 5;
pub const OP_DISABLE: u32 = 7;
pub const OP_OUTPUT_READ: u32 = 0x12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HalCommand {
    Configure,
    Enable { context: u32 },
    Disable { context: u32 },
    OutputRead { context: u32, blocks: u32 },
    Other { op: u32 },
}

impl HalCommand {
    /// `payload` is the title's payload read as little-endian words (two are enough).
    pub fn parse(op: u32, payload: [u32; 2]) -> HalCommand {
        match op {
            OP_CONFIGURE => HalCommand::Configure,
            OP_ENABLE => HalCommand::Enable {
                context: payload[0],
            },
            OP_DISABLE => HalCommand::Disable {
                context: payload[0],
            },
            OP_OUTPUT_READ => HalCommand::OutputRead {
                context: payload[0],
                blocks: payload[1],
            },
            op => HalCommand::Other { op },
        }
    }
}

/// What the title has asked of the hardware, kept so a decoder can follow it.
#[derive(Debug, Default)]
pub struct Hal {
    started: bool,
    enabled: std::collections::BTreeSet<u32>,
    /// Blocks the title reported reading, per context, since the last [`Hal::take_reads`].
    reads: std::collections::BTreeMap<u32, u32>,
    commands: u64,
}

impl Hal {
    pub fn start(&mut self) {
        self.started = true;
    }

    pub fn stop(&mut self) {
        self.started = false;
        self.enabled.clear();
        self.reads.clear();
    }

    pub fn started(&self) -> bool {
        self.started
    }

    pub fn commands(&self) -> u64 {
        self.commands
    }

    pub fn submit(&mut self, command: HalCommand) {
        self.commands += 1;
        match command {
            HalCommand::Enable { context } => {
                self.enabled.insert(context);
            }
            HalCommand::Disable { context } => {
                self.enabled.remove(&context);
            }
            HalCommand::OutputRead { context, blocks } => {
                *self.reads.entry(context).or_insert(0) += blocks;
            }
            HalCommand::Configure | HalCommand::Other { .. } => {}
        }
    }

    pub fn is_enabled(&self, context: u32) -> bool {
        self.enabled.contains(&context)
    }

    pub fn take_reads(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.reads).into_iter().collect()
    }
}

#[cfg(windows)]
pub mod object {
    use super::HalCommand;
    use crate::audio::engine;

    const S_OK: i32 = 0;
    const E_NOTIMPL: i32 = 0x8000_4001u32 as i32;

    #[repr(C)]
    pub struct Vtable {
        init: extern "win64" fn(*mut Object, u64, u64) -> i32,
        stop: extern "win64" fn(*mut Object),
        submit: extern "win64" fn(*mut Object, u32, u32, i32, *const u32, u64) -> i32,
        unused3: extern "win64" fn(*mut Object) -> i32,
        unused4: extern "win64" fn(*mut Object) -> i32,
        release: extern "win64" fn(*mut Object),
    }

    /// The object the title holds: a vtable pointer first, as for any COM-shaped object.
    #[repr(C)]
    pub struct Object {
        vtable: &'static Vtable,
    }

    static VTABLE: Vtable = Vtable {
        init,
        stop,
        submit,
        unused3,
        unused4,
        release,
    };

    pub fn new() -> Box<Object> {
        Box::new(Object { vtable: &VTABLE })
    }

    extern "win64" fn init(_this: *mut Object, a: u64, b: u64) -> i32 {
        let mut e = engine();
        e.note_once("hal-init", || {
            format!("hardware layer slot 0 called with {a:#x}, {b:#x}: taken as a start request")
        });
        e.hal_mut().start();
        S_OK
    }

    extern "win64" fn stop(_this: *mut Object) {
        engine().stop_hal();
    }

    extern "win64" fn submit(
        _this: *mut Object,
        op: u32,
        key: u32,
        _flags: i32,
        payload: *const u32,
        _extra: u64,
    ) -> i32 {
        let words = if payload.is_null() {
            [0, 0]
        } else {
            // SAFETY: the title passes the address of a payload of at least one word, and the commands that
            // carry two words (0x12) build both.
            let first = unsafe { payload.read_unaligned() };
            let second = if op == super::OP_OUTPUT_READ {
                // SAFETY: that payload is two words.
                unsafe { payload.add(1).read_unaligned() }
            } else {
                0
            };
            [first, second]
        };
        let command = HalCommand::parse(op, words);
        let mut e = engine();
        if let HalCommand::Other { op } = command {
            e.note_once(&format!("hal-op-{op:#x}"), || {
                format!("hardware layer command {op:#x} (key {key:#x}) is not one the title has been seen to use")
            });
        }
        e.apply_hal(command);
        S_OK
    }

    extern "win64" fn unused3(_this: *mut Object) -> i32 {
        E_NOTIMPL
    }

    extern "win64" fn unused4(_this: *mut Object) -> i32 {
        E_NOTIMPL
    }

    extern "win64" fn release(_this: *mut Object) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_operations_parse_by_payload() {
        assert_eq!(HalCommand::parse(2, [0xc0, 0]), HalCommand::Configure);
        assert_eq!(
            HalCommand::parse(5, [9, 0]),
            HalCommand::Enable { context: 9 }
        );
        assert_eq!(
            HalCommand::parse(7, [9, 0]),
            HalCommand::Disable { context: 9 }
        );
        assert_eq!(
            HalCommand::parse(0x12, [9, 3]),
            HalCommand::OutputRead {
                context: 9,
                blocks: 3
            }
        );
        assert_eq!(HalCommand::parse(99, [0, 0]), HalCommand::Other { op: 99 });
    }

    #[test]
    fn enable_disable_and_reads_accumulate() {
        let mut hal = Hal::default();
        hal.start();
        hal.submit(HalCommand::Enable { context: 4 });
        assert!(hal.is_enabled(4));
        hal.submit(HalCommand::OutputRead {
            context: 4,
            blocks: 3,
        });
        hal.submit(HalCommand::OutputRead {
            context: 4,
            blocks: 2,
        });
        assert_eq!(hal.take_reads(), vec![(4, 5)]);
        assert!(hal.take_reads().is_empty());
        hal.submit(HalCommand::Disable { context: 4 });
        assert!(!hal.is_enabled(4));
        assert_eq!(hal.commands(), 4);
        hal.submit(HalCommand::Enable { context: 1 });
        hal.stop();
        assert!(!hal.started() && !hal.is_enabled(1));
    }
}
