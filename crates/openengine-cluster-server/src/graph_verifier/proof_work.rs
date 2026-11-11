use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// 8 Mi count-cell operations bound all correlated assignment expansion for one verification.
/// Clones share the same accounting owner across residual alternatives.
/// This is an engine engineering limit, not an authored budget or elapsed-time timeout.
#[derive(Clone, Default)]
pub struct ProofWork(Arc<AtomicU64>);
impl ProofWork {
    pub const LIMIT: u64 = 8 * 1024 * 1024;
    /// Charge the shared structural rule before cloning a runtime predicate.
    pub fn charge_guard(&self, guard: &super::Guard) -> Option<()> {
        self.charge(1 + super::guard_node_count(guard) as usize)
    }
    pub fn charge(&self, cells: usize) -> Option<()> {
        self.0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                used.checked_add(cells as u64)
                    .filter(|next| *next <= Self::LIMIT)
            })
            .ok()?;
        Some(())
    }
}
