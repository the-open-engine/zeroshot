//! Allocation, cancellation cleanup and retained handoffs serialize only their own runs.
use std::collections::BTreeMap;
use std::sync::{Arc, Weak};

use openengine_cluster_protocol::RunId;
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::native_v2_platform::{CapsuleAllocationUnavailable, RetainedAllocationUnavailable};
use super::{
    ProductionCapsuleAllocator, RetainedAllocationPaths, read_recovery, retained_claim_failure,
    failed_run_directory_state, FailedRunDirectoryState,
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
    ) -> Result<(RunId, OwnedMutexGuard<()>), RetainedAllocationUnavailable> {
        let (source_turn, successor_turn) = self
            .allocation_turns
            .lock_retained(source, successor)
            .await?;
        if self.active.lock().await.contains_key(successor)
            || self.allocated.lock().await.contains(successor)
        {
            return Err(CapsuleAllocationUnavailable::Runtime.into());
        }
        // Resume reserves and moves the workspace before asynchronous preparation. Reuse only
        // that exact durable handoff; never claim another source or a different successor.
        if let Some(document) = read_recovery(&paths.source_recovery)
            && document.successor_run_id.as_ref() == Some(successor)
        {
            let next =
                read_recovery(&paths.run_recovery).ok_or(CapsuleAllocationUnavailable::Runtime)?;
            let delivery_run_id = document
                .delivery_run_id
                .clone()
                .ok_or(CapsuleAllocationUnavailable::Runtime)?;
            if document.recoverable
                || document.run_id.as_ref() != Some(source)
                || next.recoverable
                || next.run_id.as_ref() != Some(successor)
                || next.resumed_from.as_ref() != Some(source)
                || next.delivery_run_id.as_ref() != Some(&delivery_run_id)
                || next.successor_run_id.is_some()
                || paths.source_root.exists()
                || !matches!(
                    failed_run_directory_state(&paths.run_root)?,
                    FailedRunDirectoryState::Retained
                )
            {
                return Err(CapsuleAllocationUnavailable::Runtime.into());
            }
            return Ok((delivery_run_id, successor_turn));
        }
        let claim = self.claim_retained_allocation(source, successor, paths)?;
        if std::fs::rename(&paths.source_root, &paths.run_root).is_err() {
            return Err(retained_claim_failure(paths, &claim.original_source));
        }
        // The source is durably claimed. Only the successor owns the moved workspace now.
        drop(source_turn);
        Ok((claim.delivery_run_id, successor_turn))
    }
}
