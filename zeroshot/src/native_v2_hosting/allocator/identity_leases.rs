use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use openengine_cluster_server::graph_verifier::FULL_V1_MAX_NODE_EXECUTIONS;

use crate::execution::process::HostedProcessPool;
use crate::native_v2_runner::AUXILIARY_PROVIDER_SLOT_OFFSET;

// Primary reducer references and built-in global agents occupy disjoint bounded ranges.
const MAXIMUM_PROCESS_IDENTITY: u64 = AUXILIARY_PROVIDER_SLOT_OFFSET + FULL_V1_MAX_NODE_EXECUTIONS;

#[derive(Clone)]
pub(super) struct ActiveRunProcessPools {
    seed: HostedProcessPool,
    occupied: Arc<Mutex<BTreeSet<u32>>>,
}

impl ActiveRunProcessPools {
    pub(super) fn new(seed: HostedProcessPool) -> Result<Self, IdentityLeaseUnavailable> {
        seed.active_run_slot(0, MAXIMUM_PROCESS_IDENTITY)
            .map_err(|_| IdentityLeaseUnavailable)?;
        Ok(Self {
            seed,
            occupied: Arc::new(Mutex::new(BTreeSet::new())),
        })
    }

    pub(super) fn acquire(&self) -> Result<ActiveRunProcessPool, IdentityLeaseUnavailable> {
        let mut occupied = self.occupied.lock().map_err(|_| IdentityLeaseUnavailable)?;
        let mut slot = 0_u32;
        for current in occupied.iter().copied() {
            if current != slot {
                break;
            }
            slot = slot.checked_add(1).ok_or(IdentityLeaseUnavailable)?;
        }
        let process_pool = self
            .seed
            .active_run_slot(slot, MAXIMUM_PROCESS_IDENTITY)
            .map_err(|_| IdentityLeaseUnavailable)?;
        occupied.insert(slot);
        Ok(ActiveRunProcessPool {
            slot,
            process_pool,
            occupied: self.occupied.clone(),
        })
    }
}

pub(super) struct ActiveRunProcessPool {
    slot: u32,
    process_pool: HostedProcessPool,
    occupied: Arc<Mutex<BTreeSet<u32>>>,
}

impl ActiveRunProcessPool {
    pub(super) const fn process_pool(&self) -> HostedProcessPool {
        self.process_pool
    }
}

impl Drop for ActiveRunProcessPool {
    fn drop(&mut self) {
        let mut occupied = self
            .occupied
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        occupied.remove(&self.slot);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct IdentityLeaseUnavailable;

#[cfg(all(test, target_os = "linux"))]
#[path = "identity_leases/tests.rs"]
mod tests;
