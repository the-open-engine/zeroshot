//! Couples filesystem lifetime to confirmed contained-process cleanup.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use crate::execution::process::{ProcessRunnerError, ProcessSession, ProcessSessionOutput};

use super::filesystem::ProviderExecutionFiles;

pub(crate) struct ProviderProcess {
    process: ProcessSession,
    files: Arc<ProviderExecutionFiles>,
    pending: bool,
    retain_files: bool,
}

impl ProviderProcess {
    pub(super) fn new(process: ProcessSession, files: Arc<ProviderExecutionFiles>) -> Self {
        Self {
            process,
            files,
            pending: true,
            retain_files: false,
        }
    }

    pub(crate) async fn wait(&mut self) -> Result<ProcessSessionOutput, ProcessRunnerError> {
        let result = self.process.wait().await;
        self.record_completion(&result);
        result
    }

    pub(crate) async fn release(&mut self) -> Result<ProcessSessionOutput, ProcessRunnerError> {
        let result = self.process.release().await;
        self.record_completion(&result);
        result
    }

    /// A native runtime may own detached jobs beyond the contained process group. Keep its
    /// execution files until whole-run cleanup when that additional settlement is unconfirmed.
    #[cfg(feature = "contributed-harnesses")]
    pub(crate) fn retain_execution_files(&mut self) {
        self.retain_files = true;
    }

    fn record_completion(&mut self, result: &Result<ProcessSessionOutput, ProcessRunnerError>) {
        if self.pending
            && !self.retain_files
            && result
                .as_ref()
                .is_ok_and(|output| output.cleanup.proves_tree_empty())
        {
            self.files.process_reaped();
            self.pending = false;
        }
    }
}

impl Deref for ProviderProcess {
    type Target = ProcessSession;

    fn deref(&self) -> &Self::Target {
        &self.process
    }
}

impl DerefMut for ProviderProcess {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.process
    }
}
