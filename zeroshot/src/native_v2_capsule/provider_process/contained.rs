//! Couples filesystem lifetime to confirmed contained-process cleanup.

use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use crate::execution::process::{ProcessRunnerError, ProcessSession, ProcessSessionOutput};

use super::filesystem::ProviderExecutionFiles;

pub(crate) struct ProviderProcess {
    process: ProcessSession,
    files: Arc<ProviderExecutionFiles>,
    pending: bool,
}

impl ProviderProcess {
    pub(super) fn new(process: ProcessSession, files: Arc<ProviderExecutionFiles>) -> Self {
        Self {
            process,
            files,
            pending: true,
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

    fn record_completion(&mut self, result: &Result<ProcessSessionOutput, ProcessRunnerError>) {
        if self.pending
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
