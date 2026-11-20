//! Bounded private controller failure reports, independent of the run ledger.
//!
//! The detached controller discards its standard streams. A controller that fails before
//! readiness instead leaves one bounded, control-safe report in its run storage, redacted of the
//! credentials its bootstrap carried, and the waiting CLI consumes it. Runtime failures reuse
//! that formatting and private-file handling, retaining their note for later read-only inspection.

use std::error::Error;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use openengine_cluster_protocol::GitTransportCredentials;

use crate::native_v2_capsule::provider_process::{
    private_shell_values, redaction_values, safe_provider_text,
};
use crate::native_v2_runner::bounded_log_text;

use super::super::{PortableControllerBootstrap, PortableControllerError, PortableControllerPaths};
use super::{read_bounded_regular_file, write_private_new_file};

const MAX_FAILURE_BYTES: usize = 4 * 1024;
/// Credential values a startup failure report must not contain.
pub(crate) fn startup_redactions(bootstrap: &PortableControllerBootstrap) -> Vec<String> {
    let connections = bootstrap.environment.bootstrap_values();
    let git_secret = bootstrap
        .git_credentials
        .as_ref()
        .map(|credentials| match credentials {
            GitTransportCredentials::Https { password, .. } => password.as_str(),
            GitTransportCredentials::Ssh { private_key, .. } => private_key.as_str(),
        });
    redaction_values(
        connections
            .values()
            .flat_map(|values| values.as_map().values().map(String::as_str))
            .chain(private_shell_values(&bootstrap.native_environment))
            .chain(bootstrap.github_token.as_deref())
            .chain(git_secret),
    )
}

/// The bootstrap is written into its run storage, which receives any startup failure report.
pub(super) fn startup_paths(bootstrap_path: &Path) -> PortableControllerPaths {
    PortableControllerPaths::new(bootstrap_path.parent().unwrap_or(bootstrap_path))
}

/// Records a failure from before the bootstrap was read, such as the controller's console guard.
/// Such failures carry no bootstrap credentials.
pub fn record_unloaded_startup_failure(bootstrap_path: &Path, error: &PortableControllerError) {
    record_startup_failure(&startup_paths(bootstrap_path), error, &[]);
}

/// Records a startup failure once. Reporting is best effort and never replaces the failure.
pub(super) fn record_startup_failure(
    paths: &PortableControllerPaths,
    error: &PortableControllerError,
    redactions: &[String],
) {
    write_failure(&paths.startup_failure(), &error_chain(error), redactions);
}

/// Local runtime reporting owns no outcome or recovery authority. Claim the original cause
/// before scheduling I/O so reporting cannot delay cleanup or retry after a failed write.
pub(crate) struct LocalRuntimeFailureReport {
    paths: PortableControllerPaths,
    redactions: Vec<String>,
    writer: OnceLock<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl LocalRuntimeFailureReport {
    pub(crate) fn new(paths: PortableControllerPaths, redactions: Vec<String>) -> Self {
        Self {
            paths,
            redactions,
            writer: OnceLock::new(),
        }
    }

    pub(crate) fn record(self: &Arc<Self>, cause: &str) {
        self.writer.get_or_init(|| {
            let report = self.clone();
            let cause = cause.to_owned();
            // No file I/O stands between the original failure and process cleanup.
            tokio::sync::Mutex::new(Some(tokio::task::spawn_blocking(move || {
                write_failure(&report.paths.runtime_failure(), &cause, &report.redactions);
            })))
        });
    }

    /// Fatal process exits do not unwind Tokio. After cleanup, let a started report finish
    /// before returning that error to the executable; reporting failures stay best effort.
    pub(crate) async fn wait_written(&self) {
        if let Some(writer) = self.writer.get()
            && let Some(writer) = writer.lock().await.take()
        {
            let _ = writer.await;
        }
    }
}

fn write_failure(path: &Path, cause: &str, redactions: &[String]) {
    let report = bounded_log_text(safe_provider_text(cause, redactions), MAX_FAILURE_BYTES);
    let _ = write_private_new_file(path, report.as_bytes());
}

/// Consumes the bounded reason a controller recorded before it exited without readiness.
#[must_use]
pub fn take_startup_failure(paths: &PortableControllerPaths) -> Option<String> {
    let path = paths.startup_failure();
    let report = read_failure(&path);
    let _ = std::fs::remove_file(&path);
    report
}

/// Reads a retained runtime failure without changing the note, even if it is invalid.
pub(crate) fn read_runtime_failure(paths: &PortableControllerPaths) -> Option<String> {
    read_failure(&paths.runtime_failure())
}

fn read_failure(path: &Path) -> Option<String> {
    let bytes = read_bounded_regular_file(path, MAX_FAILURE_BYTES as u64);
    let report = String::from_utf8(bytes.ok()?).ok()?;
    // The reason joins a one-line CLI error, so line breaks and tabs become spaces too.
    let report = bounded_log_text(report, MAX_FAILURE_BYTES).replace(['\n', '\t'], " ");
    let report = report.trim();
    (!report.is_empty()).then(|| report.to_owned())
}

pub(super) fn error_chain(error: &PortableControllerError) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.ends_with(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

#[cfg(test)]
#[path = "startup_failure/tests.rs"]
mod tests;
