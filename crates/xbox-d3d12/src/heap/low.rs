//! Descriptor memory from the resource step's address space.

use std::sync::{Arc, Mutex};

use super::console::{LowBlock, LowMemory};
use crate::memory::SyncPolicy;
use crate::resource::Resources;

/// `LowMemory` over `Resources::alloc_low`: committed host memory under 4 GiB, bound into the GPU's address
/// space, where the CPU pointer is the address.
pub struct ResourcesLow {
    resources: Arc<Mutex<Resources>>,
    policy: SyncPolicy,
}

impl ResourcesLow {
    pub fn new(resources: Arc<Mutex<Resources>>, policy: SyncPolicy) -> ResourcesLow {
        ResourcesLow { resources, policy }
    }
}

struct Lease {
    resources: Arc<Mutex<Resources>>,
    address: u64,
    bytes: u64,
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut resources) = self.resources.lock() {
            resources.free_low(self.address, self.bytes);
        }
    }
}

impl LowMemory for ResourcesLow {
    fn alloc_low(&self, bytes: u64) -> Option<LowBlock> {
        let address = self
            .resources
            .lock()
            .ok()?
            .alloc_low(bytes, self.policy)
            .ok()?;
        Some(LowBlock {
            address,
            keep: Box::new(Lease {
                resources: self.resources.clone(),
                address,
                bytes,
            }),
        })
    }
}
