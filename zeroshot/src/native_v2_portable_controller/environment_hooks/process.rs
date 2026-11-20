use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::execution::WorkspaceAccessMode;
use crate::execution::driver::{DriverCancellation, WorkspaceCapability};
use crate::execution::process::{
    LocalProcessRunner, ProcessRunnerError, ProcessSession, ProcessSessionCommand,
};
use crate::native_v2_platform::hook_output::{HookOutput, HookOutputStream};
use crate::native_v2_platform::{CapsuleAllocationUnavailable, CapsuleCleanupUnavailable};

use super::{HookStop, PREPARATION_PHASE_TIMEOUT};

const BASH: &str = "/bin/bash";
// The wrapper stays alive to keep native services under the process supervisor's ownership. The
// authored Bash sees null stdin, leaving the wrapper's lifetime channel exclusively to the host.
const STARTUP_WRAPPER: &str = r#"
exec 2>&1
status=0
"$0" --noprofile --norc -p -e -o pipefail -c "$1" </dev/null || status=$?
printf '%d\n' "$status" > "$2" || exit 125
IFS= read -r ignored || :
"#;

#[derive(Clone)]
pub(super) struct HookShell {
    pub(super) workspace: PathBuf,
    pub(super) readiness: PathBuf,
    pub(super) environment: BTreeMap<String, String>,
}

pub(super) struct HookProcess {
    session: ProcessSession,
    cancellation: watch::Sender<bool>,
    output: JoinHandle<Result<(), CapsuleAllocationUnavailable>>,
}

impl HookShell {
    pub(super) async fn launch(
        &self,
        script: &str,
        deadline: Option<Instant>,
        output: HookOutput,
    ) -> Result<HookProcess, ProcessRunnerError> {
        let argv = if deadline.is_some() {
            vec![
                "-e".to_owned(),
                "-o".to_owned(),
                "pipefail".to_owned(),
                "-c".to_owned(),
                format!("exec 2>&1\n{script}"),
            ]
        } else {
            let readiness = self.readiness.to_str().ok_or_else(|| {
                ProcessRunnerError::InvalidCommand("startup status path is not UTF-8".to_owned())
            })?;
            vec![
                "-c".to_owned(),
                STARTUP_WRAPPER.to_owned(),
                BASH.to_owned(),
                script.to_owned(),
                readiness.to_owned(),
            ]
        };
        let (cancellation, cancelled) = watch::channel(false);
        let mut session = LocalProcessRunner::new()
            .open(
                ProcessSessionCommand {
                    program: BASH.to_owned(),
                    argv: ["--noprofile", "--norc", "-p"]
                        .into_iter()
                        .map(str::to_owned)
                        .chain(argv)
                        .collect(),
                    environment: self.environment.clone(),
                    workspace: WorkspaceCapability {
                        current_dir: self.workspace.clone(),
                        mode: WorkspaceAccessMode::ReadWrite,
                    },
                    deadline,
                },
                DriverCancellation::new(cancelled),
            )
            .await?;
        if deadline.is_some() {
            // A short-lived process can settle before the null input is acknowledged.
            let _ = session.close_stdin().await;
        }
        let mut stdout = session.detach_stdout();
        let output = tokio::spawn(async move {
            let mut stream = HookOutputStream::new(output);
            let mut result = Ok(());
            while let Some(chunk) = stdout.recv().await {
                if result.is_ok() {
                    result = stream.write(chunk.as_slice()).await;
                }
            }
            result?;
            stream.finish().await
        });
        Ok(HookProcess {
            session,
            cancellation,
            output,
        })
    }
}

#[derive(Clone, Copy)]
pub(super) enum StartupStatus {
    Reported(u8),
    Stopped,
    TimedOut,
    Missing,
}

impl StartupStatus {
    pub(super) fn detail(self) -> &'static str {
        match self {
            Self::Stopped => "was cancelled",
            Self::TimedOut => "timed out",
            Self::Missing | Self::Reported(_) => "exited without a startup status",
        }
    }
}

pub(super) enum HookExit {
    Code(i32),
    Interrupted(&'static str),
}

impl HookProcess {
    pub(super) fn cancellation(&self) -> watch::Sender<bool> {
        self.cancellation.clone()
    }

    pub(super) fn stop(&self) {
        self.cancellation.send_replace(true);
    }

    pub(super) async fn startup_status(
        &mut self,
        path: &Path,
        stopping: HookStop,
    ) -> StartupStatus {
        let mut interval = tokio::time::interval(Duration::from_millis(25));
        let poll = async {
            loop {
                interval.tick().await;
                match read_startup_status(path) {
                    Ok(Some(code)) => return StartupStatus::Reported(code),
                    Ok(None) => {}
                    Err(_) => return StartupStatus::Missing,
                }
            }
        };
        tokio::select! {
            biased;
            () = stopping.wait() => StartupStatus::Stopped,
            () = tokio::time::sleep(PREPARATION_PHASE_TIMEOUT) => StartupStatus::TimedOut,
            _ = self.session.wait() => StartupStatus::Missing,
            status = poll => status,
        }
    }

    pub(super) async fn finish(mut self) -> Result<HookExit, CapsuleCleanupUnavailable> {
        let result = self.session.wait().await;
        let output = match tokio::time::timeout(Duration::from_secs(5), &mut self.output).await {
            Ok(result) => matches!(result, Ok(Ok(()))),
            Err(_) => {
                self.output.abort();
                let _ = self.output.await;
                false
            }
        };
        let settled = result.map_err(|_| CapsuleCleanupUnavailable)?;
        if !settled.cleanup.proves_tree_empty() {
            return Err(CapsuleCleanupUnavailable);
        }
        if settled.timed_out {
            Ok(HookExit::Interrupted("timed out"))
        } else if settled.cancelled {
            Ok(HookExit::Interrupted("was cancelled"))
        } else if !output || settled.post_launch_error.is_some() {
            Ok(HookExit::Interrupted(
                "failed while collecting process output",
            ))
        } else if let Some(code) = settled.exit_code {
            Ok(HookExit::Code(code))
        } else {
            Ok(HookExit::Interrupted("terminated before completing"))
        }
    }
}

fn read_startup_status(path: &Path) -> io::Result<Option<u8>> {
    let opened = std::fs::File::open(path);
    if opened
        .as_ref()
        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
    {
        return Ok(None);
    }
    let mut record = String::new();
    opened?.take(5).read_to_string(&mut record)?;
    Ok(record
        .strip_suffix('\n')
        .and_then(|value| value.parse().ok()))
}
