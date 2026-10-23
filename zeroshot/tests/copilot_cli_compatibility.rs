//! Local `zeroshot run` against an installed GitHub Copilot CLI release.
//!
//! Copilot CLI 1.0.70 and 1.0.86 both report headless protocol 3 from `connect`, but only 1.0.86
//! implements `session.detach`; 1.0.70 answers it with JSON-RPC -32601. A Copilot node's
//! completed, valid answer must not be discarded because of that difference: either the run keeps
//! the accepted answer, or the incompatible CLI is refused before it receives any node work.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Output};
use std::time::Duration;

use openengine_cluster_testkit::TemporaryDirectory;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::timeout;

const RUN_TIMEOUT: Duration = Duration::from_secs(120);
const CLI_TIMEOUT: Duration = Duration::from_secs(30);

/// Observable headless-protocol behavior of one Copilot CLI release.
struct CopilotRelease {
    version: &'static str,
    /// The `connect` result the real release returns.
    connect: &'static str,
    supports_detach: bool,
}

/// The reporter's installed release: protocol 3, no `session.detach`.
const OUTDATED: CopilotRelease = CopilotRelease {
    version: "1.0.70",
    connect: r#"{"ok":true,"protocolVersion":3,"version":"1.0.70"}"#,
    supports_detach: false,
};

/// The release Zeroshot pins in its target image and install guide.
const PINNED: CopilotRelease = CopilotRelease {
    version: "1.0.86",
    connect: r#"{"ok":true,"protocolVersion":3,"version":"1.0.86","taskKinds":["agent","shell"]}"#,
    supports_detach: true,
};

#[tokio::test(flavor = "multi_thread")]
async fn outdated_copilot_cli_never_discards_an_accepted_answer() {
    let fixture = CopilotFixture::new(&OUTDATED);
    let output = fixture.run().await;
    let requests = fixture.requests();
    let prompts = prompt_count(&requests);
    let report = fixture.report(&output, &requests).await;

    if output.status.success() {
        assert_eq!(
            prompts, 1,
            "a successful Copilot run must keep the node's first accepted answer\n{report}"
        );
        return;
    }
    assert_eq!(
        prompts, 0,
        "Copilot CLI {} received the node prompt and returned a valid {{\"response\":null}} \
         answer, then Zeroshot failed the run and discarded that completed work. Keep the \
         accepted answer, or refuse the incompatible CLI before its node receives any work.\n\
         {report}",
        OUTDATED.version
    );
    let visible = fixture.visible_text(&output).await.to_ascii_lowercase();
    assert!(
        visible.lines().any(|line| line.contains("copilot")
            && [OUTDATED.version, "update", "upgrade", "install"]
                .iter()
                .any(|hint| line.contains(hint))),
        "the run refused Copilot CLI {} without telling the user to update it\n{report}",
        OUTDATED.version
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pinned_copilot_cli_keeps_the_accepted_answer() {
    let fixture = CopilotFixture::new(&PINNED);
    let output = fixture.run().await;
    let requests = fixture.requests();
    let report = fixture.report(&output, &requests).await;
    assert!(
        output.status.success(),
        "Copilot CLI {} must complete the single-worker run\n{report}",
        PINNED.version
    );
    assert_eq!(
        prompt_count(&requests),
        1,
        "the node's first answer must complete the run\n{report}"
    );
}

fn prompt_count(requests: &[String]) -> usize {
    requests
        .iter()
        .filter(|method| *method == "session.send")
        .count()
}

struct CopilotFixture {
    _root: TemporaryDirectory,
    bin: PathBuf,
    home: PathBuf,
    repository: PathBuf,
    state: PathBuf,
    config: PathBuf,
    input: PathBuf,
    runtime: PathBuf,
    capture: PathBuf,
}

impl CopilotFixture {
    fn new(release: &CopilotRelease) -> Self {
        require_restic_sidecar();
        let root = TemporaryDirectory::for_test("zcpc");
        let fixture = Self {
            bin: root.path("bin"),
            home: root.path("home"),
            repository: root.path("repository"),
            state: root.path("state"),
            config: root.path("config"),
            input: root.path("input.json"),
            runtime: root.path("runtime.json"),
            capture: root.path("requests.log"),
            _root: root,
        };
        for directory in [
            &fixture.bin,
            &fixture.home.join(".copilot"),
            &fixture.repository,
            &fixture.config,
        ] {
            fs::create_dir_all(directory).assert_value_with("create fixture directory");
        }
        openengine_cluster_testkit::fixture::write_executable(
            &fixture.bin.join("copilot"),
            fake_copilot(release, &fixture.capture),
            0o755,
        )
        .assert_value_with("write fake Copilot CLI");
        initialize_repository(&fixture.repository);
        write_json(
            &fixture.input,
            &json!({"task": "Say hello. Make no changes."}),
        );
        write_json(
            &fixture.runtime,
            // This fixture counts the main node's accepted answer; optional global work has
            // separate lifecycle and live-harness coverage.
            &json!({"harness": "copilot", "provider": "github", "model": "auto",
                "activeMemory":{"repository":false,"graph":false}}),
        );
        fixture
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_zeroshot"));
        command
            .args(args)
            .current_dir(&self.repository)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
            .env("HOME", &self.home)
            .env("ZEROSHOT_STATE_DIR", &self.state)
            .env("ZEROSHOT_CONFIG_DIR", &self.config)
            .kill_on_drop(true);
        command
    }

    async fn output(&self, args: &[&str], deadline: Duration) -> Output {
        timeout(deadline, self.command(args).output())
            .await
            .assert_value_with("zeroshot CLI deadline")
            .assert_value_with("start zeroshot CLI")
    }

    async fn run(&self) -> Output {
        self.output(
            &[
                "run",
                "--title",
                "Harness CLI compatibility",
                "--template",
                "single-worker",
                "--input",
                self.input.to_str().assert_value_with("input path"),
                "--uniform-runtime-config",
                self.runtime.to_str().assert_value_with("runtime path"),
            ],
            RUN_TIMEOUT,
        )
        .await
    }

    /// JSON-RPC methods the fake Copilot CLI received, in order.
    fn requests(&self) -> Vec<String> {
        fs::read_to_string(&self.capture)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.strip_prefix("request "))
            .map(str::to_owned)
            .collect()
    }

    fn run_id(output: &Output) -> Option<String> {
        let first = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .map(str::to_owned)?;
        let receipt: Value = serde_json::from_str(&first).ok()?;
        receipt["runId"].as_str().map(str::to_owned)
    }

    async fn logs(&self, output: &Output) -> String {
        let Some(run_id) = Self::run_id(output) else {
            return String::new();
        };
        match timeout(CLI_TIMEOUT, self.command(&["logs", &run_id]).output()).await {
            Ok(Ok(logs)) => String::from_utf8_lossy(&logs.stdout).into_owned(),
            Ok(Err(error)) => format!("<zeroshot logs failed to start: {error}>"),
            Err(_) => "<zeroshot logs timed out>".to_owned(),
        }
    }

    /// Everything `zeroshot run` and `zeroshot logs` show the user about this run.
    async fn visible_text(&self, output: &Output) -> String {
        format!(
            "{}\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            self.logs(output).await
        )
    }

    async fn report(&self, output: &Output, requests: &[String]) -> String {
        format!(
            "run status: {}\nCopilot requests: {requests:?}\nrun stdout:\n{}\nrun stderr:\n{}\n\
             zeroshot logs:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            self.logs(output).await
        )
    }
}

impl Drop for CopilotFixture {
    fn drop(&mut self) {
        let Ok(entries) = fs::read_dir(self.state.join("runs")) else {
            return;
        };
        for ready in entries
            .flatten()
            .map(|entry| entry.path().join("controller.ready.json"))
        {
            let pid = fs::read(ready)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|value| value.get("pid").and_then(Value::as_u64))
                .and_then(|pid| i32::try_from(pid).ok());
            if let Some(pid) = pid {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
    }
}

/// A headless stdio peer that answers like the given Copilot CLI release. Frames, events, and
/// replies follow `zeroshot/src/native_v2_copilot/tests/runtime.py`; the `--version`, `connect`,
/// and `session.detach` replies are the ones the real 1.0.70 and 1.0.86 CLIs return.
fn fake_copilot(release: &CopilotRelease, capture: &Path) -> String {
    let detach = if release.supports_detach {
        r#"reply "$id" '{"success":true}'"#
    } else {
        r#"unhandled "$id" "$method""#
    };
    FAKE_COPILOT
        .replace("@VERSION@", release.version)
        .replace("@CONNECT@", release.connect)
        .replace("@CAPTURE@", &capture.display().to_string())
        .replace("@DETACH@", detach)
}

const FAKE_COPILOT: &str = r#"#!/bin/sh
version='@VERSION@'
capture='@CAPTURE@'
printf 'launch %s\n' "$*" >> "$capture"
case "${1-}" in
  --version|-v|version)
    printf 'GitHub Copilot CLI %s.\n' "$version"
    printf '%s\n' "Run 'copilot update' to check for updates."
    exit 0
    ;;
esac

send() {
  printf 'Content-Length: %s\r\n\r\n%s' "$(printf '%s' "$1" | wc -c | tr -d ' ')" "$1"
}
reply() {
  send '{"jsonrpc":"2.0","id":'"$1"',"result":'"$2"'}'
}
unhandled() {
  send '{"jsonrpc":"2.0","id":'"$1"',"error":{"code":-32601,"message":"Unhandled method '"$2"'"}}'
}
event() {
  send '{"jsonrpc":"2.0","method":"session.event","params":{"sessionId":"'"$session"'","event":{"type":'"$1"'}}}'
}
first() {
  printf '%s' "$body" | grep -o "$1" | head -n 1
}
read_frame() {
  length=
  while IFS= read -r line; do
    line=$(printf '%s' "$line" | tr -d '\r')
    if [ -z "$line" ]; then
      break
    fi
    case "$line" in
      [Cc]ontent-[Ll]ength:*) length=$(printf '%s' "${line#*:}" | tr -d ' ') ;;
    esac
  done
  if [ -z "$length" ]; then
    return 1
  fi
  body=$(dd bs=1 count="$length" 2>/dev/null)
}

session=
while read_frame; do
  method=$(first '"method":"[^"]*"' | cut -d '"' -f 4)
  id=$(first '"id":[0-9][0-9]*' | cut -d : -f 2)
  printf 'request %s\n' "$method" >> "$capture"
  if [ -z "$method" ] || [ -z "$id" ]; then
    continue
  fi
  case "$method" in
    connect)
      reply "$id" '@CONNECT@'
      ;;
    session.create|session.resume)
      session=$(first '"sessionId":"[^"]*"' | cut -d '"' -f 4)
      reply "$id" '{"sessionId":"'"$session"'"}'
      ;;
    session.send)
      event '"assistant.usage","data":{"inputTokens":11,"outputTokens":5}'
      event '"assistant.message","data":{"content":"{\"response\":null}"}'
      reply "$id" '{"messageId":"message-1"}'
      ;;
    session.detach)
      @DETACH@
      ;;
    *)
      unhandled "$id" "$method"
      ;;
  esac
done
"#;

/// Local runs checkpoint the workspace before any node starts, and the local controller finds
/// Restic only beside its executable or on its `PATH`. Without it every run fails before Copilot
/// is launched, which would say nothing about Copilot.
fn require_restic_sidecar() {
    let sibling = Path::new(env!("CARGO_BIN_EXE_zeroshot")).with_file_name("restic");
    let on_path = ["/usr/bin/restic", "/bin/restic"]
        .iter()
        .any(|candidate| Path::new(candidate).is_file());
    assert!(
        sibling.is_file() || on_path,
        "setup: place the pinned Restic sidecar at {} (CI: \"Build pinned Restic sidecar for \
         native tests\")",
        sibling.display()
    );
}

fn initialize_repository(repository: &Path) {
    git(repository, &["init"]);
    git(repository, &["config", "user.name", "Harness CLI Test"]);
    git(
        repository,
        &["config", "user.email", "harness-cli@example.invalid"],
    );
    git(repository, &["config", "commit.gpgsign", "false"]);
    fs::write(repository.join("README.md"), b"hello\n").assert_value_with("seed repository");
    git(repository, &["add", "README.md"]);
    git(repository, &["commit", "-m", "seed"]);
    git(repository, &["branch", "-M", "main"]);
    git(
        repository,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/harness-fixture.git",
        ],
    );
}

fn git(repository: &Path, arguments: &[&str]) {
    let output = StdCommand::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", repository)
        .output()
        .assert_value_with("run Git fixture command");
    assert!(
        output.status.success(),
        "Git fixture command {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_json(path: &Path, value: &Value) {
    fs::write(
        path,
        serde_json::to_vec(value).assert_value_with("encode fixture JSON"),
    )
    .assert_value_with("write fixture JSON");
}
