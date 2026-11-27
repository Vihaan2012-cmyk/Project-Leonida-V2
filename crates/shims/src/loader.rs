//! Run-time library loading (LoadLibrary*, GetModuleHandle*, GetProcAddress, FreeLibrary) for console DLLs, and the GDK runtime behind xgameruntime.dll.

use crate::Shim;

pub fn shims() -> Vec<Shim> {
    Vec::new()
}
