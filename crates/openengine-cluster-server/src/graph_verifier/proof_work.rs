use std::cell::Cell;

/// 8 Mi count-cell operations bound all correlated assignment expansion for one verification.
/// This is an engine engineering limit, not an authored budget or elapsed-time timeout.
#[derive(Default)]
pub(in crate::graph_verifier) struct ProofWork(Cell<u64>);
impl ProofWork {
    pub(in crate::graph_verifier) const LIMIT: u64 = 8 * 1024 * 1024;
    pub(in crate::graph_verifier) fn charge(&self, cells: usize) -> Option<()> {
        let next = self.0.get().checked_add(cells as u64)?;
        if next > Self::LIMIT {
            return None;
        }
        self.0.set(next);
        Some(())
    }
}
