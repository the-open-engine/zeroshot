//! Local review bindings survive incomplete delivery as part of workspace recovery metadata.

use super::*;
use crate::full_v1_reducer::DurableExecutionState;
use crate::v2_run_ledger::StoredRun;
use crate::native_v2_delivery::{DeliveryMode, ReviewBindingRecorder, contract};

/// The controller owns this document until it releases the workspace; successor claims only
/// update it after that release. Reuse the lifecycle's atomic, durable private-file writes.
pub(crate) fn review_binding_recorder(
    storage: &Path,
    workspace: &Path,
    branch: Option<&str>,
    delivery_run_id: &RunId,
) -> Result<Option<ReviewBindingRecorder>, NativeV2CliError> {
    let path = storage.join(RECOVERY_FILE);
    let Some(branch) = branch else {
        return Ok(None);
    };
    if !path.try_exists().map_err(local_io)? {
        // In-process embedders do not own a local CLI recovery document.
        return Ok(None);
    }
    let workspace = workspace.to_owned();
    let branch = branch.to_owned();
    let delivery_run_id = delivery_run_id.clone();
    let record = move |review: &crate::source_code_provider::SourceReviewReceipt| {
        let mut recovery = LocalRecoveryDocument::read(&path)?;
        if recovery.workspace != workspace
            || recovery.local_branch.as_deref() != Some(&branch)
            || recovery.delivery_run_id.as_ref() != Some(&delivery_run_id)
        {
            return Err(local_message(
                "delivery review binding differs from the retained workspace",
            ));
        }
        let mode = crate::native_v2_admission::delivery_mode(&recovery.submission.graph.root)
            .filter(|mode| *mode != DeliveryMode::Push)
            .ok_or_else(|| local_message("review binding requires review delivery"))?;
        let target = recovery_target(&recovery, mode)?;
        let receipt = contract::review_binding_receipt(mode, review);
        if review.head_branch != branch
            || contract::review_from_receipt(&receipt, mode, &target, &branch).as_ref()
                != Some(review)
            || recovery.delivery_receipt.as_ref().is_some_and(|existing| {
                contract::review_from_receipt(existing, mode, &target, &branch)
                    .is_none_or(|known| known.review_id != review.review_id)
            })
        {
            return Err(local_message(
                "confirmed review differs from the retained delivery identity",
            ));
        }
        if recovery.delivery_receipt.as_ref() == Some(&receipt) {
            return Ok(());
        }
        recovery.delivery_receipt = Some(receipt);
        recovery.write(&path)
    };
    Ok(Some(Arc::new(move |review| {
        record(review).map_err(|error| error.to_string())
    })))
}

fn recovery_target(
    recovery: &LocalRecoveryDocument,
    mode: DeliveryMode,
) -> Result<crate::source_code_provider::DeliveryTarget, NativeV2CliError> {
    crate::native_v2_delivery::source_target(
        recovery
            .submission
            .source
            .as_ref()
            .ok_or_else(|| local_message("delivery recovery source is missing"))?,
        mode,
    )
    .map_err(local_error)
}

impl LocalCliBackend {
    pub(super) async fn recover_delivery_receipt(
        &self,
        predecessor: &RunId,
        prepared: &PreparedLocalRun,
    ) -> Result<Option<serde_json::Value>, NativeV2CliError> {
        let Some(branch) = prepared.local_branch.as_deref() else {
            return Ok(None);
        };
        let mut run_id = predecessor.clone();
        for _ in 0..MAX_RECOVERY_LINEAGE_DEPTH {
            let recovery = self.read_recovery_document(&run_id)?;
            if recovery.workspace != prepared.workspace
                || recovery.local_branch.as_deref() != Some(branch)
                || recovery
                    .delivery_run_id
                    .as_ref()
                    .is_some_and(|id| id != &prepared.delivery_run_id)
            {
                return Err(local_message(
                    "delivery recovery lineage differs from the retained workspace",
                ));
            }
            if let Some(receipt) = &recovery.delivery_receipt {
                let mode =
                    crate::native_v2_admission::delivery_mode(&recovery.submission.graph.root)
                        .ok_or_else(|| local_message("delivery recovery mode is missing"))?;
                let target = recovery_target(&recovery, mode)?;
                contract::review_from_receipt(receipt, mode, &target, branch)
                    .ok_or_else(|| local_message("retained delivery review identity is invalid"))?;
                return Ok(Some(receipt.clone()));
            }
            let path = self.paths(&run_id)?.ledger();
            if require_existing_ledger(&path)? {
                let ledger = SqliteRunLedger::open_read_only(&path).map_err(local_error)?;
                let stored = ledger
                    .get(&run_id)
                    .await
                    .map_err(local_error)?
                    .ok_or_else(|| {
                        local_message("delivery recovery ledger does not contain its run")
                    })?;
                if let Some(receipt) = recorded_review(&stored, branch)? {
                    return Ok(Some(receipt));
                }
            }
            let Some(predecessor) = recovery.resumed_from else {
                return Ok(None);
            };
            run_id = predecessor;
        }
        Err(local_message("workspace recovery lineage is too deep"))
    }
}

fn recorded_review(
    stored: &StoredRun,
    branch: &str,
) -> Result<Option<serde_json::Value>, NativeV2CliError> {
    let Some((node, mode)) = crate::native_v2_admission::sole_delivery_node(&stored.admitted)
    else {
        return Ok(None);
    };
    let target = crate::native_v2_delivery::source_target(
        stored
            .admitted
            .source
            .as_ref()
            .ok_or_else(|| local_message("delivery recovery source is missing"))?,
        mode,
    )
    .map_err(local_error)?;
    let history =
        crate::native_v2_supervisor::durable_history(&stored.snapshot).map_err(local_error)?;
    Ok(history
        .iter()
        .filter(|execution| execution.occurrence.node == node)
        .filter_map(|execution| {
            let DurableExecutionState::Settled {
                position,
                outcome: openengine_cluster_protocol::WorkerOutcome::Verifier { output, .. },
            } = &execution.state
            else {
                return None;
            };
            crate::native_v2_delivery::contract::review_from_receipt(output, mode, &target, branch)
                .map(|_| (*position, output))
        })
        .max_by_key(|(position, _)| *position)
        .map(|(_, output)| output.clone()))
}

#[cfg(test)]
#[path = "delivery_recovery_tests.rs"]
mod tests;
