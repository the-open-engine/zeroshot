use std::collections::{BTreeMap, BTreeSet};

use crate::native_v2_observability::lineage::{LineageError, RunLineage, part_cursor};
use super::*;

impl NativeV2PlatformController {
    pub(crate) async fn run_lineage(
        &self,
        run_id: &RunId,
    ) -> Result<RunLineage, NativeV2PlatformError> {
        RunLineage::resolve(run_id, |part| self.observability.recovery_for_part(part))
            .await
            .map_err(|error| match error {
                LineageError::Read(error) => NativeV2PlatformError::Observation(error),
                LineageError::Cycle => RunLedgerError::Corrupt.into(),
            })
    }

    pub(in crate::native_v2_platform) async fn run_families(
        &self,
    ) -> Result<Vec<RunLineage>, NativeV2PlatformError> {
        let mut included = BTreeSet::new();
        let inventory = self.list().await?;
        let mut families = BTreeMap::new();
        for entry in &inventory {
            if included.contains(&entry.run_id) {
                continue;
            }
            let family = self.run_lineage(&entry.run_id).await?;
            included.extend(family.parts().iter().cloned());
            // A handoff may commit between two inventory reads.
            for child in family.parts().iter().skip(1) {
                families.remove(child);
            }
            families.insert(family.root().clone(), family);
        }
        Ok(inventory
            .iter()
            .filter_map(|entry| families.remove(&entry.run_id))
            .collect())
    }

    pub(super) async fn projected_status(
        &self,
        family: &RunLineage,
        part_id: Option<&RunId>,
    ) -> Result<RunStatusResult, NativeV2PlatformError> {
        let selected = selected_part(family, part_id)?;
        let mut result = self.status(RunStatusParams::new(selected.clone())).await?;
        result.run_id = family.root().clone();
        result.current_part_id = family.current().clone();
        result.part_id = selected.clone();
        family.project_recovery(selected, &mut result.workspace_recovery);
        if part_id.is_none() {
            self.aggregate_status(family, &mut result.status, &mut result.at_cursor)
                .await?;
        }
        Ok(result)
    }

    pub(super) async fn aggregate_status(
        &self,
        family: &RunLineage,
        status: &mut RunStatus,
        cursor: &mut openengine_cluster_protocol::Cursor,
    ) -> Result<(), NativeV2PlatformError> {
        *cursor = part_cursor(family.current(), cursor);
        crate::native_v2_observability::lineage::scope_diagnostic_cursors(family.current(), status);
        if let RunStatus::Finished { metadata, .. } | RunStatus::Suspended { metadata, .. } = status
        {
            for id in family.parts().iter().take(family.parts().len() - 1) {
                let stored = self
                    .ledger
                    .get(id)
                    .await?
                    .ok_or(RunLedgerError::RunNotFound)?;
                crate::v2_run_ledger::merge_usage(
                    &mut metadata.token_usage,
                    stored.snapshot.token_usage.as_ref(),
                )?;
            }
        }
        Ok(())
    }

    pub(in crate::native_v2_platform) async fn ensure_current_part(
        &self,
        expected: Option<&RunId>,
    ) -> Result<(), NativeV2PlatformError> {
        let Some(expected) = expected else {
            return Ok(());
        };
        let family = self.run_lineage(expected).await?;
        if family.current() != expected {
            return Err(NativeV2PlatformError::StalePart(family.current().clone()));
        }
        Ok(())
    }

    /// Serialize a guarded mutation with the physical source's workspace reservation.
    pub(in crate::native_v2_platform) async fn lock_observed_part(
        &self,
        run: &RunId,
        expected: Option<&RunId>,
    ) -> Result<Option<tokio::sync::OwnedMutexGuard<()>>, NativeV2PlatformError> {
        let Some(expected) = expected else {
            return Ok(None);
        };
        let guard = self.lock_run_control(run).await;
        self.ensure_current_part(Some(expected)).await?;
        Ok(Some(guard))
    }

    pub(super) async fn read_target(
        &self,
        run: &RunId,
        selected: Option<&RunId>,
    ) -> Result<(RunLineage, RunId), BackendError> {
        let family = self
            .run_lineage(run)
            .await
            .map_err(platform_backend_error)?;
        let part = selected_part(&family, selected)
            .map_err(platform_backend_error)?
            .clone();
        Ok((family, part))
    }

    pub(super) async fn current_target(
        &self,
        run: &RunId,
        expected: Option<&RunId>,
    ) -> Result<(RunLineage, RunId), BackendError> {
        let family = self
            .run_lineage(run)
            .await
            .map_err(platform_backend_error)?;
        if expected.is_some_and(|expected| expected != family.current()) {
            return Err(platform_backend_error(NativeV2PlatformError::StalePart(
                family.current().clone(),
            )));
        }
        let current = family.current().clone();
        Ok((family, current))
    }
}

pub(super) fn selected_part<'f>(
    family: &'f RunLineage,
    part_id: Option<&'f RunId>,
) -> Result<&'f RunId, NativeV2PlatformError> {
    let part = part_id.unwrap_or_else(|| family.current());
    if family.contains(part) {
        Ok(part)
    } else {
        Err(NativeV2PlatformError::InvalidRequest(
            "partId does not belong to the requested run".into(),
        ))
    }
}
