//! Adapter-owned CLI compatibility, checked without starting a model session.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use crate::execution::WorkspaceAccessMode;
use crate::execution::driver::{DriverCancellation, WorkspaceCapability};
use crate::execution::process::{LocalProcessRunner, ProcessSessionCommand};
use crate::native_v2_capsule::provider_process::{
    LocalHarnessEnvironment, process_failure_evidence, safe_provider_text, redaction_values,
    require_process_cleanup,
};

pub(crate) const CHECK_BUDGET: Duration = Duration::from_secs(60);
pub(crate) const MAX_REPORTED_BYTES: usize = 1024;

/// Keep bounded raw bytes for classification, but never publish a truncated credential prefix.
#[derive(Default)]
pub(crate) struct VersionOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

impl VersionOutput {
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        let room = MAX_REPORTED_BYTES.saturating_sub(self.bytes.len());
        self.truncated |= bytes.len() > room;
        self.bytes
            .extend_from_slice(&bytes[..bytes.len().min(room)]);
    }

    pub(crate) fn accepted_version(&self, range: &HarnessCliVersion) -> Option<[u64; 3]> {
        range.accepts(String::from_utf8_lossy(&self.bytes).trim())
    }

    pub(crate) fn diagnostic(&self) -> Cow<'_, str> {
        if self.truncated {
            Cow::Borrowed("<omitted: CLI version output exceeded the byte limit>")
        } else {
            String::from_utf8_lossy(&self.bytes)
        }
    }
}

/// A maximum is exclusive and is set only for a known incompatible release.
/// Native output syntax, environment selection and upgrade instructions belong to the adapter.
#[derive(Clone, Copy)]
pub struct HarnessCliVersion {
    pub minimum: [u64; 3],
    pub first_unsupported: Option<[u64; 3]>,
    pub arguments: &'static [&'static str],
    pub parse: fn(&str) -> Option<[u64; 3]>,
    pub upgrade: &'static str,
    pub local_environment: fn(&LocalHarnessEnvironment) -> BTreeMap<String, String>,
}

pub(crate) struct LocalCliContext<'a> {
    pub(crate) executable: &'a Path,
    pub(crate) native: &'a LocalHarnessEnvironment,
    pub(crate) search_path: &'a str,
    pub(crate) workspace: &'a Path,
    pub(crate) stopping: Option<tokio::sync::watch::Receiver<bool>>,
}

impl HarnessCliVersion {
    pub(crate) fn accepts(&self, reported: &str) -> Option<[u64; 3]> {
        (self.parse)(reported).filter(|version| {
            *version >= self.minimum
                && self
                    .first_unsupported
                    .is_none_or(|maximum| *version < maximum)
        })
    }

    pub(crate) fn refusal(&self, lane: &str, executable: &Path, reported: &str) -> String {
        let minimum = display_release(self.minimum);
        let range = self.first_unsupported.map_or_else(
            || format!("{minimum} or newer"),
            |maximum| format!(">={minimum}, <{}", display_release(maximum)),
        );
        format!(
            "lane {lane} needs CLI {range}; executable {}: the executable's {} reported {:?}; {}",
            executable.display(),
            self.arguments.join(" "),
            reported.lines().next().unwrap_or_default(),
            self.upgrade,
        )
    }

    pub(crate) async fn check_local(
        &self,
        lane: &str,
        context: LocalCliContext<'_>,
    ) -> Result<(), String> {
        let LocalCliContext {
            executable,
            native,
            search_path,
            workspace,
            stopping,
        } = context;
        let mut environment = (self.local_environment)(native);
        for name in ["LANG", "LC_ALL", "TERM", "TMPDIR"] {
            if let Some(value) = native.get(name) {
                environment.insert(name.to_owned(), value.clone());
            }
        }
        environment.insert("PATH".to_owned(), search_path.to_owned());
        if let Some(home) = native.user_home() {
            environment.insert("HOME".to_owned(), home.clone());
        }
        let redactions = redaction_values(environment.values().map(String::as_str));
        let command = ProcessSessionCommand {
            program: executable.to_string_lossy().into_owned(),
            argv: self.arguments.iter().map(|arg| (*arg).to_owned()).collect(),
            environment,
            workspace: WorkspaceCapability {
                current_dir: workspace.to_owned(),
                mode: WorkspaceAccessMode::ReadWrite,
            },
            deadline: Some(tokio::time::Instant::now() + CHECK_BUDGET),
        };
        let (reported, failure) = probe(command, stopping).await;
        if failure.is_none() && reported.accepted_version(self).is_some() {
            return Ok(());
        }
        let reported = safe_provider_text(reported.diagnostic().trim(), &redactions);
        let mut detail = self.refusal(lane, executable, &reported);
        if let Some(failure) = failure {
            detail.push_str("; ");
            detail.push_str(&safe_provider_text(&failure, &redactions));
        }
        Err(detail)
    }
}

async fn probe(
    command: ProcessSessionCommand,
    stopping: Option<tokio::sync::watch::Receiver<bool>>,
) -> (VersionOutput, Option<String>) {
    let (_request, cancellation) = tokio::sync::watch::channel(false);
    let mut process = match LocalProcessRunner::new()
        .open(
            command,
            DriverCancellation::new(stopping.unwrap_or(cancellation)),
        )
        .await
    {
        Ok(process) => process,
        Err(error) => return (VersionOutput::default(), Some(error.to_string())),
    };
    let _ = process.close_stdin().await;
    let mut stdout = process.detach_stdout();
    let mut reported = VersionOutput::default();
    while let Some(chunk) = stdout.recv().await {
        reported.push(chunk.as_slice());
    }
    let completion = process.wait().await;
    let failure = match require_process_cleanup(&completion) {
        Ok(output) if output.cancelled => Some("CLI version probe was cancelled".to_owned()),
        Ok(output) => process_failure_evidence(output, false).map(|failure| failure.detail),
        Err(_) => Some("CLI version probe cleanup could not be confirmed".to_owned()),
    };
    (reported, failure)
}

pub(crate) fn display_release([major, minor, patch]: [u64; 3]) -> String {
    format!("{major}.{minor}.{patch}")
}

/// Reads a numeric leading MAJOR.MINOR.PATCH, rejecting unknown/prerelease output.
pub(crate) fn release(reported: &str) -> Option<[u64; 3]> {
    let mut parts = reported.split_whitespace().next()?.split('.').map(|part| {
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse::<u64>().ok())
            .flatten()
    });
    let release = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(release)
}

#[cfg(test)]
mod tests;
