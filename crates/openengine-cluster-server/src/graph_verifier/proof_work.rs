use std::sync::{
    atomic::{AtomicU64, Ordering},
};

/// 8 Mi count-cell operations bound all correlated assignment expansion for one verification.
/// All correlated proofs share this accounting owner within one verification.
/// This is an engine engineering limit, not an authored budget or elapsed-time timeout.
#[derive(Default)]
pub(in crate::graph_verifier) struct ProofWork(AtomicU64);
impl ProofWork {
    pub const LIMIT: u64 = 8 * 1024 * 1024;
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
