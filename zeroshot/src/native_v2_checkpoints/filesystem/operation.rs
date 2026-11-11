//! Cancellation and supervised Git for synchronous snapshot work owned by an async controller.

use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::execution::driver::DriverCancellation;
use crate::native_v2_capsule::provider_process::copy_workspace_entry_until_stopped;
use crate::native_v2_delivery::command::{GitCommandOutput, capture_sensitive_until_stopped};

pub(crate) struct SnapshotOperation {
    stopping: Option<watch::Receiver<bool>>,
    runtime: Option<Handle>,
}
impl SnapshotOperation {
    /// Created on the controller before moving filesystem work to the blocking pool.
    pub(crate) fn current(stopping: Option<watch::Receiver<bool>>) -> Self {
        Self {
            stopping,
            runtime: Some(Handle::current()),
        }
    }

    pub(super) fn uncancelled() -> Self {
        Self {
            stopping: None,
            runtime: None,
        }
    }

    fn is_stopping(&self) -> bool {
        self.stopping
            .as_ref()
            .is_some_and(|request| *request.borrow())
    }

    pub(super) fn check(&self) -> io::Result<()> {
        if self.is_stopping() {
            return Err(super::super::stopped());
        }
        Ok(())
    }

    pub(super) fn copy(&self, source: &Path, target: &Path) -> io::Result<()> {
        self.check()?;
        let request = self
            .stopping
            .clone()
            .unwrap_or_else(|| watch::channel(false).1);
        let result =
            copy_workspace_entry_until_stopped(source, target, &DriverCancellation::new(request));
        if result.is_err() && self.is_stopping() {
            return Err(super::super::stopped());
        }
        result
    }

    pub(super) fn git(&self, command: Command) -> io::Result<GitCommandOutput> {
        self.check()?;
        let stopping = self.stopping.clone();
        let run = async move {
            let mut command = tokio::process::Command::from(command);
            capture_sensitive_until_stopped(&mut command, Duration::from_secs(120), &[], stopping)
                .await
        };
        let result = if let Some(runtime) = &self.runtime {
            runtime.block_on(run)
        } else {
            // Synchronous embedders may themselves be on a runtime thread. Keep their API while
            // giving the shared runner an independently driven I/O runtime, never nesting one.
            std::thread::Builder::new()
                .name("checkpoint-git".to_owned())
                .spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map(|runtime| runtime.block_on(run))
                })?
                .join()
                .map_err(|_| io::Error::other("checkpoint Git task failed"))??
        };
        match result {
            Ok(output) if !output.stdout_valid_utf8() => {
                Err(io::Error::other("invalid UTF-8 in Git checkpoint output"))
            }
            Ok(output) if !output.stdout_truncated() => Ok(output),
            Ok(_) => Err(io::Error::other("Git checkpoint output exceeded its bound")),
            Err(failure) if !failure.cleanup_unconfirmed && self.is_stopping() => {
                Err(super::super::stopped())
            }
            Err(failure) => Err(io::Error::other(if failure.cleanup_unconfirmed {
                "Git checkpoint command failed; its process cleanup is unconfirmed"
            } else {
                "Git checkpoint command failed"
            })),
        }
    }
}
