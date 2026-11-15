//! Compact cleanup ownership belongs to live provider sessions, independently of retry IDs.
use super::*;
use std::collections::BTreeSet;

pub(crate) const MAX_PROCESS_SLOTS: u64 = 131_072;

#[derive(Clone, Default)]
pub(super) struct ProcessSlots(Arc<std::sync::Mutex<ProcessSlotState>>);
#[derive(Default)]
struct ProcessSlotState {
    free: BTreeSet<u64>,
    next_slot: u64,
    next_home: u64,
}
impl ProcessSlots {
    pub(super) fn acquire(&self) -> Result<Arc<ProcessSlot>, NodeRunnerError> {
        let mut occupied = self.0.lock().map_err(|_| NodeRunnerError::SessionOpen)?;
        let home_number = occupied
            .next_home
            .checked_add(1)
            .ok_or(NodeRunnerError::SessionOpen)?;
        let home = NodeInstanceId::new(home_number).map_err(|_| NodeRunnerError::SessionOpen)?;
        let slot = match occupied.free.pop_first() {
            Some(slot) => slot,
            None => {
                let slot = occupied
                    .next_slot
                    .checked_add(1)
                    .filter(|slot| *slot <= MAX_PROCESS_SLOTS)
                    .ok_or(NodeRunnerError::SessionOpen)?;
                occupied.next_slot = slot;
                slot
            }
        };
        occupied.next_home = home_number;
        Ok(Arc::new(ProcessSlot {
            slot: NodeInstanceId::new(slot).map_err(|_| NodeRunnerError::SessionOpen)?,
            home,
            pool: self.clone(),
            quarantined: AtomicBool::new(false),
            released: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            active: std::sync::atomic::AtomicUsize::new(0),
        }))
    }
}

pub(super) struct ProcessSlot {
    pub(super) slot: NodeInstanceId,
    pub(super) home: NodeInstanceId,
    pool: ProcessSlots,
    quarantined: AtomicBool,
    released: AtomicBool,
    closed: AtomicBool,
    active: std::sync::atomic::AtomicUsize,
}
impl ProcessSlot {
    pub(super) fn quarantine(&self) {
        self.quarantined.store(true, Ordering::SeqCst);
    }
    pub(super) fn begin(&self) -> Result<(), NodeRunnerError> {
        self.active.fetch_add(1, Ordering::SeqCst);
        if self.closed.load(Ordering::SeqCst) || self.released.load(Ordering::SeqCst) {
            self.end();
            return Err(NodeRunnerError::SessionLost);
        }
        Ok(())
    }
    pub(super) fn end(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.release_if_settled();
    }
    pub(super) fn release(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.release_if_settled();
    }
    fn release_if_settled(&self) {
        if self.closed.load(Ordering::SeqCst)
            && self.active.load(Ordering::SeqCst) == 0
            && !self.quarantined.load(Ordering::SeqCst)
            && !self.released.swap(true, Ordering::SeqCst)
        {
            self.pool
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .free
                .insert(self.slot.get());
        }
    }
}

#[cfg(test)]
mod tests;
