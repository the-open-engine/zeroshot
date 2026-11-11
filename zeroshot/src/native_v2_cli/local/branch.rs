//! Detached-HEAD diagnostics reuse retained local recovery metadata, without restoring refs.

use std::{collections::BTreeSet, path::Path};

use crate::native_v2_local::{LocalCompositionError, resolve_workspace, verified_detached_branch};

use super::{LocalCliBackend, NativeV2CliError};

impl LocalCliBackend {
    pub(super) fn local_branch_error(
        &self,
        error: LocalCompositionError,
        selected: &Path,
    ) -> NativeV2CliError {
        if matches!(
            error,
            LocalCompositionError::DetachedHead { .. } | LocalCompositionError::DetachedHeadUnknown
        ) && let Some(branch) = self.retained_detached_branch(selected)
        {
            return NativeV2CliError::Usage(
                LocalCompositionError::DetachedHead { branch }.to_string(),
            );
        }
        NativeV2CliError::Usage(error.to_string())
    }

    fn retained_detached_branch(&self, selected: &Path) -> Option<String> {
        let selected = self.current_directory.join(selected);
        let workspace = resolve_workspace(&selected, &self.git_program).ok()?;
        let branches: BTreeSet<_> = self
            .local_run_ids()
            .ok()?
            .iter()
            .filter_map(|run_id| self.read_recovery_document(run_id).ok())
            .filter(|recovery| {
                std::fs::canonicalize(&recovery.workspace).ok().as_ref() == Some(&workspace)
            })
            .filter_map(|recovery| recovery.local_branch)
            .filter(|branch| verified_detached_branch(&workspace, &self.git_program, branch))
            .collect();
        if branches.len() == 1 {
            branches.into_iter().next()
        } else {
            None
        }
    }
}
