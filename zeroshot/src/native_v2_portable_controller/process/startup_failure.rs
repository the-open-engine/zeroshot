//! The reason a private controller exited before readiness.
//!
//! The detached controller discards its standard streams. A controller that fails before
//! readiness instead leaves one bounded, control-safe report in its run storage, redacted of the
//! credentials its bootstrap carried, and the waiting CLI consumes it.

use std::error::Error;

use openengine_cluster_protocol::GitTransportCredentials;

use crate::native_v2_capsule::provider_process::{
    private_local_values, redaction_values, safe_provider_text,
};
use crate::native_v2_runner::bounded_log_text;

use super::super::{PortableControllerBootstrap, PortableControllerError, PortableControllerPaths};
use super::{read_bounded_regular_file, write_private_new_file};

const MAX_STARTUP_FAILURE_BYTES: usize = 4 * 1024;

/// Credential values a startup failure report must not contain.
pub(super) fn startup_redactions(bootstrap: &PortableControllerBootstrap) -> Vec<String> {
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
            .chain(private_local_values(&bootstrap.native_environment))
            .chain(bootstrap.github_token.as_deref())
            .chain(git_secret),
    )
}

/// Records a startup failure once. Reporting is best effort and never replaces the failure.
pub(super) fn record_startup_failure(
    paths: &PortableControllerPaths,
    error: &PortableControllerError,
    redactions: &[String],
) {
    let report = bounded_log_text(
        safe_provider_text(&error_chain(error), redactions),
        MAX_STARTUP_FAILURE_BYTES,
    );
    let _ = write_private_new_file(&paths.startup_failure(), report.as_bytes());
}

/// Consumes the bounded reason a controller recorded before it exited without readiness.
#[must_use]
pub fn take_startup_failure(paths: &PortableControllerPaths) -> Option<String> {
    let path = paths.startup_failure();
    let bytes = read_bounded_regular_file(&path, MAX_STARTUP_FAILURE_BYTES as u64);
    let _ = std::fs::remove_file(&path);
    let report = String::from_utf8(bytes.ok()?).ok()?;
    let report = bounded_log_text(report, MAX_STARTUP_FAILURE_BYTES);
    (!report.trim().is_empty()).then_some(report)
}

fn error_chain(error: &PortableControllerError) -> String {
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
