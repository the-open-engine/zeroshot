//! Allocation, cancellation cleanup and retained handoffs serialize only their own runs.
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

use openengine_cluster_protocol::RunId;
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::native_v2_cloud::{CapsuleAllocationUnavailable, RetainedAllocationUnavailable};
use super::{
    ProductionCapsuleAllocator, RetainedAllocationClaim, RetainedAllocationPaths,
    retained_claim_failure,
};

#[derive(Default)]
pub(super) struct AllocationTurns {
    runs: Mutex<BTreeMap<RunId, Weak<Mutex<()>>>>,
}

impl AllocationTurns {
    pub async fn lock(&self, run_id: &RunId) -> OwnedMutexGuard<()> {
        let turn = {
            let mut runs = self.runs.lock().await;
            runs.retain(|_, turn| turn.strong_count() > 0);
            if let Some(turn) = runs.get(run_id).and_then(Weak::upgrade) {
                turn
            } else {
                let turn = Arc::new(Mutex::new(()));
                runs.insert(run_id.clone(), Arc::downgrade(&turn));
                turn
            }
        };
        turn.lock_owned().await
    }

    pub async fn lock_retained(
        &self,
        source: &RunId,
        successor: &RunId,
    ) -> Result<(OwnedMutexGuard<()>, OwnedMutexGuard<()>), CapsuleAllocationUnavailable> {
        // Stable ordering also protects against opposing handoffs without a global queue.
        match source.cmp(successor) {
            std::cmp::Ordering::Less => {
                let source_turn = self.lock(source).await;
                Ok((source_turn, self.lock(successor).await))
            }
            std::cmp::Ordering::Greater => {
                let successor_turn = self.lock(successor).await;
                Ok((self.lock(source).await, successor_turn))
            }
            std::cmp::Ordering::Equal => Err(CapsuleAllocationUnavailable::Runtime),
        }
    }
}

impl ProductionCapsuleAllocator {
    pub(super) async fn begin_retained_allocation(
        &self,
        source: &RunId,
        successor: &RunId,
        paths: &RetainedAllocationPaths,
    ) -> Result<(RetainedAllocationClaim, OwnedMutexGuard<()>), RetainedAllocationUnavailable> {
        let (source_turn, successor_turn) = self
            .allocation_turns
            .lock_retained(source, successor)
            .await?;
        let claim = self.claim_retained_allocation(source, successor, paths)?;
        if std::fs::rename(&paths.source_root, &paths.run_root).is_err() {
            return Err(retained_claim_failure(paths, &claim.original_source));
        }
        // The source is durably claimed. Only the successor owns the moved workspace now.
        drop(source_turn);
        Ok((claim, successor_turn))
    }
}
