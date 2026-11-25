//! Translate logical run operations before calling part-local controller and ledger operations.
use super::*;
use crate::native_v2_observability::lineage::{
    part_cursor, scope_diagnostic_cursors, split_cursor, LineageError, RunLineage,
};
use crate::v2_run_ledger::{RunLedgerError, StoredRun};
use openengine_cluster_protocol::{RunStatus, TokenUsage};

impl LocalCliBackend {
    /// A proposed handoff needs independent evidence that the controller admitted this part.
    pub(crate) async fn part_recovery(
        &self,
        id: RunId,
    ) -> Result<Option<WorkspaceRecovery>, NativeV2CliError> {
        let (paths, _directory) = match self.recorded_directory(&id) {
            Ok(storage) => storage,
            Err(NativeV2CliError::RunNotFound { .. }) => return Ok(None),
            Err(error) => return Err(error),
        };
        // History remains readable if recovery metadata is missing or damaged.
        // A mutation checks the authoritative recovery document separately.
        let Ok(document) = self.read_recovery_document(&id) else {
            return Ok(None);
        };
        // Only an admitted source may propose a successor. A new child has just a backlink
        // until admission publishes its ledger row or its controller readiness.
        if document.resumed_from.is_some()
            && document.successor_run_id.is_none()
            && !read_ready(&paths).is_ok_and(|ready| ready.run_id == id)
            && !self
                .local_successor_was_admitted(&paths.ledger(), &id)
                .await?
        {
            return Ok(None);
        }
        Ok(Some(WorkspaceRecovery {
            resumed_from: document.resumed_from,
            successor_run_id: document.successor_run_id,
            ..Default::default()
        }))
    }

    pub(super) async fn local_successor_was_admitted(
        &self,
        ledger_path: &Path,
        successor_run_id: &RunId,
    ) -> Result<bool, NativeV2CliError> {
        if !require_existing_ledger(ledger_path).map_err(local_io)? {
            return Ok(false);
        }
        // SQLite creates the empty file before its first schema transaction commits.
        if std::fs::metadata(ledger_path).map_err(local_io)?.len() == 0 {
            return Ok(false);
        }
        Ok(read_part(ledger_path, successor_run_id).await?.is_some())
    }

    pub(super) async fn lineage(&self, id: &RunId) -> Result<RunLineage, NativeV2CliError> {
        RunLineage::resolve(id, |part| self.part_recovery(part))
            .await
            .map_err(|e| match e {
                LineageError::Read(error) => error,
                LineageError::Cycle => local_message("cycle in run recovery lineage"),
            })
    }

    pub(super) async fn select_part(
        &self,
        id: &RunId,
        part: Option<&RunId>,
        mutation: bool,
    ) -> Result<(RunLineage, RunId), NativeV2CliError> {
        let lineage = self.lineage(id).await?;
        let part = part.unwrap_or(lineage.current());
        if !lineage.contains(part) || (mutation && lineage.current() != part) {
            return Err(NativeV2CliError::Remote {
                code: openengine_cluster_protocol::RUN_CONFLICT.into(),
                message: format!(
                    "part {} is unavailable for this operation; current part is {}",
                    part.as_str(),
                    lineage.current().as_str()
                ),
                details: Some(serde_json::json!({"currentPartId": lineage.current()})),
            });
        }
        let selected = part.clone();
        Ok((lineage, selected))
    }

    pub(super) async fn logical_status(
        &self,
        params: RunStatusParams,
    ) -> Result<RunStatusResult, NativeV2CliError> {
        let (lineage, part) = self
            .select_part(&params.run_id, params.part_id.as_ref(), false)
            .await?;
        let result = self
            .status_local(RunStatusParams::new(part.clone()))
            .await?;
        self.project_status(&lineage, result, params.part_id.is_some())
            .await
    }

    pub(super) async fn project_status(
        &self,
        lineage: &RunLineage,
        mut result: RunStatusResult,
        exact: bool,
    ) -> Result<RunStatusResult, NativeV2CliError> {
        let part = result.part_id.clone();
        if !exact
            && let RunStatus::Finished { metadata, .. } | RunStatus::Suspended { metadata, .. } =
                &mut result.status
        {
            let previous = self.previous_usage(lineage, &part).await?;
            crate::v2_run_ledger::merge_usage(&mut metadata.token_usage, previous.as_ref())?;
        }
        lineage.project_recovery(&part, &mut result.workspace_recovery);
        result.run_id = lineage.root().clone();
        result.current_part_id = lineage.current().clone();
        if !exact {
            result.at_cursor = part_cursor(&result.part_id, &result.at_cursor);
            scope_diagnostic_cursors(&result.part_id, &mut result.status);
        }
        result.fit_published_steering()?;
        Ok(result)
    }

    pub(super) async fn project_control(
        &self,
        lineage: &RunLineage,
        result: &mut CliRunStatusResult,
    ) -> Result<(), NativeV2CliError> {
        result.run_id = lineage.root().clone();
        lineage.project_recovery(&result.part_id, &mut result.workspace_recovery);
        if split_cursor(&result.at_cursor).is_none() {
            result.at_cursor = part_cursor(&result.part_id, &result.at_cursor);
        }
        if let super::super::CliRunStatus::Target(status) = &mut result.status {
            scope_diagnostic_cursors(&result.part_id, status);
        }
        if let super::super::CliRunStatus::Target(
            RunStatus::Finished { metadata, .. } | RunStatus::Suspended { metadata, .. },
        ) = &mut result.status
        {
            let previous = self.previous_usage(lineage, &result.part_id).await?;
            crate::v2_run_ledger::merge_usage(&mut metadata.token_usage, previous.as_ref())?;
        }
        result.fit_published_steering()?;
        Ok(())
    }

    pub(super) async fn previous_usage(
        &self,
        lineage: &RunLineage,
        before: &RunId,
    ) -> Result<Option<TokenUsage>, NativeV2CliError> {
        let mut usage = None;
        for previous in lineage.parts().iter().take_while(|id| *id != before) {
            self.accumulate_part_usage(previous, &mut usage).await?;
        }
        Ok(usage)
    }

    pub(super) async fn accumulate_part_usage(
        &self,
        part: &RunId,
        usage: &mut Option<TokenUsage>,
    ) -> Result<(), NativeV2CliError> {
        let (paths, _guard) = self.recorded_directory(part)?;
        let path = paths.ledger();
        let stored = if require_existing_ledger(&path).map_err(local_io)? {
            read_part(&path, part).await
        } else {
            Err(RunLedgerError::Storage)
        };
        let part_usage = match stored {
            Ok(Some(stored)) => stored.snapshot.token_usage,
            Ok(None) => return Err(local_message("missing run part")),
            // Failure to read an already confirmed ancestor loses only its accounting.
            Err(
                RunLedgerError::Storage
                | RunLedgerError::SqliteStorage(_)
                | RunLedgerError::Corrupt,
            ) => Some(TokenUsage {
                input_tokens: Default::default(),
                output_tokens: Default::default(),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
                complete: false,
            }),
            Err(error) => return Err(error.into()),
        };
        crate::v2_run_ledger::merge_usage(usage, part_usage.as_ref())?;
        Ok(())
    }

    pub(super) async fn logical_list(&self) -> Result<RunListResult, NativeV2CliError> {
        let parts = self.list_local().await?.runs;
        let indexed = parts
            .iter()
            .map(|r| (&r.run_id, r))
            .collect::<std::collections::HashMap<_, _>>();
        let mut runs = Vec::new();
        for entry in &parts {
            let mut status = entry.clone();
            if status
                .workspace_recovery
                .successor_run_id
                .as_ref()
                .and_then(|next| indexed.get(next))
                .and_then(|next| next.workspace_recovery.resumed_from.as_ref())
                == Some(&status.run_id)
            {
                continue;
            }
            let lineage = RunLineage::resolve(&status.run_id, |part| {
                std::future::ready(Ok::<_, std::convert::Infallible>(
                    indexed
                        .get(&part)
                        .map(|status| status.workspace_recovery.clone()),
                ))
            })
            .await
            .map_err(local_error)?;
            if lineage.current() != &status.run_id {
                continue;
            }
            if let RunStatus::Finished { metadata, .. } | RunStatus::Suspended { metadata, .. } =
                &mut status.status
            {
                for previous in lineage
                    .parts()
                    .iter()
                    .take_while(|id| *id != lineage.current())
                {
                    if let RunStatus::Finished { metadata: old, .. }
                    | RunStatus::Suspended { metadata: old, .. } = &indexed[previous].status
                    {
                        crate::v2_run_ledger::merge_usage(
                            &mut metadata.token_usage,
                            old.token_usage.as_ref(),
                        )?;
                    } else {
                        self.accumulate_part_usage(previous, &mut metadata.token_usage)
                            .await?;
                    }
                }
            }
            status.run_id = lineage.root().clone();
            lineage.project_recovery(&status.part_id, &mut status.workspace_recovery);
            status.at_cursor = part_cursor(&status.part_id, &status.at_cursor);
            scope_diagnostic_cursors(&status.part_id, &mut status.status);
            runs.push(status);
        }
        runs.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        Ok(RunListResult { runs })
    }
}

async fn read_part(path: &Path, part: &RunId) -> Result<Option<StoredRun>, RunLedgerError> {
    SqliteRunLedger::open_read_only(path)?.get(part).await
}
