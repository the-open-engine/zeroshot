//! Claude Code 2.1.310 is the oldest release this adapter supports.
//!
//! The transcript relies on fields that release writes on every numbered result: per-result usage
//! in `model_usage_since_last_result` and stopped subagents in `subagent_stats`. Older releases
//! report only session running totals, which a resumed session cannot divide into invocations.
//! The selected executable is checked once per run, before its first model turn, through the
//! same launch path and environment as that turn. Local admission uses the same declaration.

use std::sync::Arc;

use super::registration::CLI_VERSION;
use crate::harnesses::cli_version::{CHECK_BUDGET, VersionOutput};
#[cfg(test)]
use crate::harnesses::cli_version::release;

use super::{ClaudeAdapter, ClaudeAttempt, ClaudeProcessStart, ClaudeTurn, turn_process};
use crate::execution::process::{ProcessSessionCommand, ProcessSessionOutput};
use crate::native_v2_capsule::provider_process::{
    ProviderExecutionFiles, open_provider_process, process_failure_evidence,
    report_cleanup_failure, require_process_cleanup,
};
use crate::native_v2_runner::{LiveOutput, LiveOutputStream, NodeRunnerError};

impl ClaudeAdapter {
    /// Returns a failed start when the executable is not a supported Claude Code release.
    pub(super) async fn require_supported_version(
        &self,
        files: Arc<ProviderExecutionFiles>,
        command: &ProcessSessionCommand,
        turn: &ClaudeTurn<'_>,
    ) -> Result<Option<ClaudeProcessStart>, NodeRunnerError> {
        let control = turn.control;
        let run_id = &turn.invocation.node.reference.run_id;
        let mut cancellation = control.cancellation();
        let mut checked_runs = tokio::select! {
            _ = cancellation.cancelled() => return Err(NodeRunnerError::Cancelled),
            checked = self.version_checked_runs.lock() => checked,
        };
        if checked_runs.contains(run_id) {
            return Ok(None);
        }
        let mut check = command.clone();
        check.argv = self.prefix_arguments.clone();
        check.argv.push("--version".to_owned());
        check.deadline = Some(tokio::time::Instant::now() + CHECK_BUDGET);
        let mut process = match open_provider_process(files, check, control).await? {
            Ok(process) => process,
            Err(error) => return turn_process::failed_before_start(error, control).map(Some),
        };
        // The check takes no input; an executable that reads stdin sees EOF at once.
        let _ = process.close_stdin().await;
        let mut stdout = process.detach_stdout();
        let mut reported = VersionOutput::default();
        while let Some(chunk) = stdout.recv().await {
            reported.push(chunk.as_slice());
        }
        let completion = process.wait().await;
        let Ok(output) = require_process_cleanup(&completion) else {
            report_cleanup_failure(
                control,
                "Claude Code version check process cleanup could not be confirmed; model turn was not started."
                    .to_owned(),
            )
            .await;
            return Err(NodeRunnerError::CleanupUnconfirmed);
        };
        if output.cancelled || control.is_cancelled() {
            return Err(NodeRunnerError::Cancelled);
        }
        if process_failure_evidence(output, false).is_none()
            && let Some([major, minor, patch]) = reported.accepted_version(&CLI_VERSION)
        {
            control
                .emit(LiveOutput::new(
                    LiveOutputStream::System,
                    format!("Claude Code version: {major}.{minor}.{patch}"),
                )?)
                .await?;
            checked_runs.insert(run_id.clone());
            return Ok(None);
        }
        Ok(Some(ClaudeProcessStart::Failed(
            ClaudeAttempt::process_failure(unsupported(
                &format!("claude/{}", self.provider.name()),
                &command.program,
                reported.diagnostic().trim(),
                output,
            )),
        )))
    }
}

fn unsupported(
    lane: &str,
    executable: &str,
    reported: &str,
    output: &ProcessSessionOutput,
) -> String {
    let mut detail = CLI_VERSION.refusal(lane, std::path::Path::new(executable), reported);
    if let Some(failure) = process_failure_evidence(output, false) {
        detail.push_str("; ");
        detail.push_str(&failure.detail);
    }
    detail
}

#[cfg(test)]
#[path = "version/tests.rs"]
mod tests;
