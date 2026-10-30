//! Supervised Restic access for private workspace checkpoint repositories.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;

use crate::execution::platform::{self, FileAccess};

const MAX_RECORD_BYTES: usize = 2 * 1024 * 1024;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(900);
const PASSWORD_BYTES: usize = 32;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ResticProgram {
    executable: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    arguments: Vec<String>,
    #[cfg(test)]
    #[serde(default)]
    in_process_test_fake: bool,
}

impl ResticProgram {
    pub(super) fn discover() -> io::Result<Self> {
        if let Some(configured) = std::env::var_os("ZEROSHOT_RESTIC") {
            return Self::from_path(PathBuf::from(configured)).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("ZEROSHOT_RESTIC does not name a restic executable: {error}"),
                )
            });
        }
        let name = if cfg!(windows) {
            "restic.exe"
        } else {
            "restic"
        };
        if let Some(parent) = std::env::current_exe()?.parent() {
            let sibling = parent.join(name);
            if sibling.is_file() {
                return Self::from_path(sibling);
            }
        }
        if let Some(search) = std::env::var_os("PATH") {
            for directory in std::env::split_paths(&search) {
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return Self::from_path(candidate);
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the bundled restic executable is unavailable; place restic beside zeroshot, on PATH, \
             or name it with ZEROSHOT_RESTIC",
        ))
    }

    fn from_path(path: PathBuf) -> io::Result<Self> {
        let executable = std::fs::canonicalize(path)?;
        let metadata = std::fs::metadata(&executable)?;
        if !metadata.is_file() {
            return Err(io::Error::other("restic executable is not a file"));
        }
        Ok(Self {
            executable,
            arguments: Vec::new(),
            #[cfg(test)]
            in_process_test_fake: false,
        })
    }

    #[cfg(test)]
    pub(super) fn test(executable: PathBuf, arguments: Vec<String>) -> io::Result<Self> {
        let mut program = Self::from_path(executable)?;
        program.arguments = arguments;
        Ok(program)
    }

    #[cfg(test)]
    pub(crate) fn fake(_directory: &Path) -> io::Result<Self> {
        Ok(Self {
            executable: PathBuf::from("in-process-test-restic"),
            arguments: Vec::new(),
            in_process_test_fake: true,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SnapshotId(String);

impl SnapshotId {
    pub(super) fn parse(value: String) -> io::Result<Self> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(io::Error::other(
                "restic returned an invalid snapshot identity",
            ));
        }
        Ok(Self(value))
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

pub(super) struct Repository {
    program: ResticProgram,
    root: PathBuf,
}

impl Repository {
    pub(super) fn new(program: ResticProgram, root: PathBuf) -> Self {
        Self { program, root }
    }

    pub(super) async fn initialize(&self) -> io::Result<()> {
        self.prepare_paths()?;
        if self
            .run(&self.root, &arguments(["cat", "config"]))
            .await
            .is_err()
        {
            self.run(&self.root, &arguments(["init"])).await?;
        }
        self.run(&self.root, &arguments(["unlock", "--remove-all"]))
            .await?;
        Ok(())
    }

    pub(super) async fn backup(
        &self,
        source: &Path,
        parent: Option<&SnapshotId>,
    ) -> io::Result<SnapshotId> {
        let mut arguments = arguments([
            "backup",
            ".",
            "--json",
            "--host",
            "zeroshot",
            "--pack-size",
            "16",
        ]);
        if let Some(parent) = parent {
            arguments.push("--parent".into());
            arguments.push(parent.as_str().into());
        }
        let output = self.run(source, &arguments).await?;
        backup_snapshot(&output)
    }

    pub(super) async fn restore(&self, snapshot: &SnapshotId, target: &Path) -> io::Result<()> {
        let arguments = [
            OsString::from("restore"),
            snapshot.as_str().into(),
            "--target".into(),
            target.as_os_str().to_owned(),
        ];
        self.run(&self.root, &arguments).await?;
        Ok(())
    }

    fn prepare_paths(&self) -> io::Result<()> {
        platform::private_directory(&self.root)?;
        platform::private_directory(&self.repository_path())?;
        self.ensure_password()
    }

    fn ensure_password(&self) -> io::Result<()> {
        let password = self.password_path();
        match platform::private_file(&password, FileAccess::Read) {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut random = [0_u8; PASSWORD_BYTES];
                getrandom::fill(&mut random).map_err(io::Error::other)?;
                let encoded = random
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                let mut file = platform::private_file(&password, FileAccess::CreateNew)?;
                file.write_all(encoded.as_bytes())?;
                file.write_all(b"\n")?;
                file.sync_all()
            }
            Err(error) => Err(error),
        }
    }

    fn repository_path(&self) -> PathBuf {
        self.root.join("repository")
    }

    fn password_path(&self) -> PathBuf {
        self.root.join("password")
    }

    async fn run(&self, directory: &Path, arguments: &[OsString]) -> io::Result<Vec<u8>> {
        #[cfg(test)]
        if self.program.in_process_test_fake {
            return self.run_test_fake(directory, arguments);
        }
        self.run_with_timeout(directory, arguments, OPERATION_TIMEOUT)
            .await
    }

    async fn run_with_timeout(
        &self,
        directory: &Path,
        arguments: &[OsString],
        timeout: Duration,
    ) -> io::Result<Vec<u8>> {
        let operation = operation_name(arguments);
        let mut environment = BTreeMap::new();
        platform::process_environment(&mut environment);
        let mut command = Command::new(&self.program.executable);
        command
            .args(&self.program.arguments)
            .arg("--no-cache")
            .args(arguments)
            .current_dir(directory)
            .env_clear()
            .envs(environment)
            .env("HOME", &self.root)
            .env("RESTIC_PASSWORD_FILE", self.password_path())
            .env("RESTIC_REPOSITORY", self.repository_path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|error| operation_io_error(operation, "start", error))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("restic stdout is unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("restic stderr is unavailable"))?;
        let result = tokio::time::timeout(timeout, async {
            tokio::try_join!(
                async {
                    operation_output(stdout, operation == "backup")
                        .await
                        .map_err(|error| operation_io_error(operation, "stdout read", error))
                },
                async {
                    discard_output(stderr)
                        .await
                        .map_err(|error| operation_io_error(operation, "stderr read", error))
                },
                async {
                    child
                        .wait()
                        .await
                        .map_err(|error| operation_io_error(operation, "wait", error))
                },
            )
        })
        .await;
        let (output, (), status) = match result {
            Ok(Ok(value)) => value,
            failure => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(match failure {
                    Ok(Err(error)) => error,
                    _ => io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("restic {operation} timed out"),
                    ),
                });
            }
        };
        if !status.success() {
            return Err(io::Error::other(format!(
                "restic {operation} failed ({status})"
            )));
        }
        Ok(output)
    }

    #[cfg(test)]
    fn run_test_fake(&self, directory: &Path, arguments: &[OsString]) -> io::Result<Vec<u8>> {
        let command = arguments
            .first()
            .and_then(|value| value.to_str())
            .ok_or_else(|| io::Error::other("fake restic command is invalid"))?;
        match command {
            "cat" => std::fs::read(self.repository_path().join("config")),
            "init" => self.initialize_test_fake(),
            "unlock" => Ok(Vec::new()),
            "backup" => self.backup_test_fake(directory),
            "restore" => self.restore_test_fake(arguments),
            _ => Err(io::Error::other("fake restic command is unsupported")),
        }
    }

    #[cfg(test)]
    fn initialize_test_fake(&self) -> io::Result<Vec<u8>> {
        platform::private_directory(&self.repository_path())?;
        std::fs::write(self.repository_path().join("config"), b"fake-restic-v1\n")?;
        Ok(Vec::new())
    }

    #[cfg(test)]
    fn backup_test_fake(&self, source: &Path) -> io::Result<Vec<u8>> {
        if self.root.join("fail-backup").exists() {
            return Err(io::Error::other("fake restic backup failed"));
        }
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let snapshot = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let snapshots = self.repository_path().join("snapshots");
        platform::private_directory(&snapshots)?;
        crate::native_v2_capsule::provider_process::copy_workspace_entry(
            source,
            &snapshots.join(&snapshot),
        )?;
        let mut output = serde_json::to_vec(&serde_json::json!({
            "message_type": "summary",
            "snapshot_id": snapshot,
        }))
        .map_err(io::Error::other)?;
        output.push(b'\n');
        Ok(output)
    }

    #[cfg(test)]
    fn restore_test_fake(&self, arguments: &[OsString]) -> io::Result<Vec<u8>> {
        if self.root.join("fail-restore").exists() {
            return Err(io::Error::other("fake restic restore failed"));
        }
        let snapshot = arguments
            .get(1)
            .and_then(|value| value.to_str())
            .ok_or_else(|| io::Error::other("fake restic snapshot is invalid"))?;
        let target_index = arguments
            .iter()
            .position(|value| value == "--target")
            .ok_or_else(|| io::Error::other("fake restic target is missing"))?;
        let target = arguments
            .get(target_index + 1)
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::other("fake restic target is missing"))?;
        copy_test_children(
            &self.repository_path().join("snapshots").join(snapshot),
            &target,
        )?;
        Ok(Vec::new())
    }
}

fn backup_snapshot(output: &[u8]) -> io::Result<SnapshotId> {
    let mut snapshot = None;
    for line in output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let value: serde_json::Value = serde_json::from_slice(line)
            .map_err(|_| io::Error::other("restic returned invalid JSON"))?;
        if value["message_type"] == "summary" {
            snapshot = value["snapshot_id"].as_str().map(str::to_owned);
        }
    }
    SnapshotId::parse(snapshot.ok_or_else(|| io::Error::other("restic did not commit a snapshot"))?)
}

#[cfg(test)]
fn copy_test_children(source: &Path, target: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        crate::native_v2_capsule::provider_process::copy_workspace_entry(
            &entry.path(),
            &target.join(entry.file_name()),
        )?;
    }
    Ok(())
}

fn arguments<const N: usize>(values: [&str; N]) -> Vec<OsString> {
    values.into_iter().map(OsString::from).collect()
}

fn operation_name(arguments: &[OsString]) -> &'static str {
    match arguments.first().and_then(|value| value.to_str()) {
        Some("cat") => "cat",
        Some("init") => "init",
        Some("unlock") => "unlock",
        Some("backup") => "backup",
        Some("restore") => "restore",
        _ => "operation",
    }
}

fn operation_io_error(operation: &str, stage: &str, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("restic {operation} {stage} failed ({:?})", error.kind()),
    )
}

async fn discard_output(mut reader: impl AsyncRead + Unpin) -> io::Result<()> {
    tokio::io::copy(&mut reader, &mut tokio::io::sink())
        .await
        .map(|_| ())
}

async fn operation_output(reader: impl AsyncRead + Unpin, backup: bool) -> io::Result<Vec<u8>> {
    if !backup {
        discard_output(reader).await?;
        return Ok(Vec::new());
    }
    let mut reader = BufReader::new(reader);
    let mut record = Vec::new();
    let mut summary = Vec::new();
    let mut oversized = false;
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            if !oversized {
                retain_summary(&record, &mut summary);
            }
            return Ok(summary);
        }
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let length = newline.unwrap_or(chunk.len());
        append_record(&mut record, &chunk[..length], &mut oversized);
        reader.consume(length + usize::from(newline.is_some()));
        if newline.is_some() {
            if !oversized {
                retain_summary(&record, &mut summary);
            }
            record.clear();
            oversized = false;
        }
    }
}

fn append_record(record: &mut Vec<u8>, fragment: &[u8], oversized: &mut bool) {
    if *oversized {
        return;
    }
    if record.len() + fragment.len() > MAX_RECORD_BYTES {
        record.clear();
        *oversized = true;
    } else {
        record.extend_from_slice(fragment);
    }
}

fn retain_summary(record: &[u8], summary: &mut Vec<u8>) {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(record) else {
        return;
    };
    if value["message_type"] == "summary" {
        summary.clear();
        summary.extend_from_slice(record);
    }
}

#[cfg(test)]
#[path = "restic/tests.rs"]
mod tests;
