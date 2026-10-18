//! The reason a private controller exited before readiness.
//!
//! The detached controller discards its standard streams. A controller that fails before
//! readiness instead leaves one bounded, control-safe report in its run storage, redacted of the
//! credentials its bootstrap carried, and the waiting CLI consumes it.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use openengine_cluster_protocol::GitTransportCredentials;

use crate::native_v2_capsule::provider_process::{
    private_local_values, redaction_values, safe_provider_text,
};
use crate::native_v2_runner::bounded_log_text;

use super::super::{PortableControllerBootstrap, PortableControllerError, PortableControllerPaths};
use super::{read_bounded_regular_file, write_private_new_file};

const MAX_STARTUP_FAILURE_BYTES: usize = 4 * 1024;
/// Shorter captured shell values are flags such as `1`; provider rules still cover named ones.
const MIN_SHELL_SECRET_BYTES: usize = 8;
/// Captured shell names known to carry no credential. Their values, such as `HOME`, often
/// appear in paths, so redacting them would hide the cause.
const PUBLIC_SHELL_NAMES: &[&str] = &[
    "APPDATA",
    "COLORTERM",
    "COMSPEC",
    "HOME",
    "LANG",
    "LOCALAPPDATA",
    "LOGNAME",
    "OLDPWD",
    "PATH",
    "PATHEXT",
    "PWD",
    "SHELL",
    "SYSTEMROOT",
    "TEMP",
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TMP",
    "TMPDIR",
    "USER",
    "USERNAME",
    "USERPROFILE",
    "WINDIR",
    "ZEROSHOT_RESTIC",
];

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
            .chain(shell_secrets(&bootstrap.native_environment))
            .chain(bootstrap.github_token.as_deref())
            .chain(git_secret),
    )
}

/// The local bootstrap captures the whole invoking shell, so its values are redacted unless their
/// name is known to be public. Unfamiliar names fail closed.
fn shell_secrets(environment: &BTreeMap<String, String>) -> impl Iterator<Item = &str> {
    environment
        .iter()
        .filter(|(name, value)| value.len() >= MIN_SHELL_SECRET_BYTES && !public_shell_name(name))
        .map(|(_, value)| value.as_str())
        .chain(private_local_values(environment))
}

fn public_shell_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.starts_with("LC_") || PUBLIC_SHELL_NAMES.contains(&name.as_str())
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
    // The reason joins a one-line CLI error, so line breaks and tabs become spaces too.
    let report = bounded_log_text(report, MAX_STARTUP_FAILURE_BYTES).replace(['\n', '\t'], " ");
    let report = report.trim();
    (!report.is_empty()).then(|| report.to_owned())
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
