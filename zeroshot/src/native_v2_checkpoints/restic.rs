//! Supervised Restic access for private workspace checkpoint repositories.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::execution::platform::{self, FileAccess};

const MAX_OUTPUT_BYTES: u64 = 2 * 1024 * 1024;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(900);
const PASSWORD_BYTES: usize = 32;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ResticProgram {
    executable: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    arguments: Vec<String>,
}

impl ResticProgram {
    pub(super) fn discover() -> io::Result<Self> {
        if let Some(configured) = std::env::var_os("ZEROSHOT_RESTIC") {
            return Self::from_path(PathBuf::from(configured));
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
            "the bundled restic executable is unavailable",
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
        })
    }

    #[cfg(test)]
    pub(super) fn test(executable: PathBuf, arguments: Vec<String>) -> io::Result<Self> {
        let mut program = Self::from_path(executable)?;
        program.arguments = arguments;
        Ok(program)
    }

    #[cfg(test)]
    pub(crate) fn fake(directory: &Path) -> io::Result<Self> {
        let script = directory.join("fake-restic.js");
        std::fs::write(
            &script,
            r#"'use strict';
const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const args = process.argv.slice(2);
if (args[0] === '--no-cache') args.shift();
const command = args[0];
const repository = process.env.RESTIC_REPOSITORY;
const snapshots = path.join(repository, 'snapshots');
const controls = path.dirname(repository);
for (const secret of ['ANTHROPIC_API_KEY', 'OPENAI_API_KEY', 'AWS_SECRET_ACCESS_KEY']) {
  if (process.env[secret] !== undefined) throw new Error(`inherited ${secret}`);
}
function copyChildren(source, target) {
  fs.mkdirSync(target, { recursive: true });
  for (const name of fs.readdirSync(source)) {
    fs.cpSync(path.join(source, name), path.join(target, name), {
      recursive: true,
      preserveTimestamps: true,
      verbatimSymlinks: true,
    });
  }
}
if (command === 'cat') {
  process.stdout.write(fs.readFileSync(path.join(repository, 'config')));
} else if (command === 'init') {
  fs.mkdirSync(repository, { recursive: true });
  fs.writeFileSync(path.join(repository, 'config'), 'fake-restic-v1\n');
} else if (command === 'unlock') {
  // The fake repository has no locks.
} else if (command === 'backup') {
  if (fs.existsSync(path.join(controls, 'fail-backup'))) process.exit(19);
  const id = crypto.randomBytes(32).toString('hex');
  copyChildren(process.cwd(), path.join(snapshots, id));
  process.stdout.write(JSON.stringify({message_type: 'summary', snapshot_id: id}) + '\n');
} else if (command === 'restore') {
  if (fs.existsSync(path.join(controls, 'fail-restore'))) process.exit(20);
  const id = args[1];
  const target = args[args.indexOf('--target') + 1];
  copyChildren(path.join(snapshots, id), target);
} else {
  process.exitCode = 2;
}
"#,
        )?;
        let name = if cfg!(windows) { "node.exe" } else { "node" };
        let executable = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
            .map(|root| root.join(name))
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "node is unavailable"))?;
        Self::test(
            executable,
            vec![
                script
                    .to_str()
                    .ok_or_else(|| io::Error::other("fake restic path is not Unicode"))?
                    .to_owned(),
            ],
        )
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
        SnapshotId::parse(
            snapshot.ok_or_else(|| io::Error::other("restic did not commit a snapshot"))?,
        )
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
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("restic stdout is unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("restic stderr is unavailable"))?;
        let result = tokio::time::timeout(OPERATION_TIMEOUT, async {
            tokio::try_join!(bounded_output(stdout), bounded_output(stderr), child.wait())
        })
        .await;
        let (output, _private_diagnostic, status) = match result {
            Ok(Ok(value)) => value,
            _ => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(io::Error::other("restic operation did not finish"));
            }
        };
        if !status.success() {
            return Err(io::Error::other("restic operation failed"));
        }
        Ok(output)
    }
}

fn arguments<const N: usize>(values: [&str; N]) -> Vec<OsString> {
    values.into_iter().map(OsString::from).collect()
}

async fn bounded_output(reader: impl tokio::io::AsyncRead + Unpin) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_OUTPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() as u64 > MAX_OUTPUT_BYTES {
        return Err(io::Error::other("restic output exceeds its size limit"));
    }
    Ok(bytes)
}
