#![cfg(unix)]

//! Local PR and ship delivery must run the GitHub CLI that the user's PATH finds.
//!
//! A local `zeroshot run --pr` or `--ship` captures the invoking shell's PATH. Delivery must start
//! the `gh` that this PATH resolves, including directories outside the system's default search
//! path, such as Homebrew's `/opt/homebrew/bin` or `~/.local/bin`. When that PATH has no `gh`, the
//! change cannot be delivered, so the run must be rejected before any agent starts.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Output};
use std::time::Duration;

use openengine_cluster_testkit::TemporaryDirectory;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::timeout;

const CLI_TIMEOUT: Duration = Duration::from_secs(90);
const DELIVERY_FLAGS: [&str; 2] = ["--pr", "--ship"];

#[tokio::test(flavor = "multi_thread")]
async fn local_delivery_starts_the_gh_that_the_users_path_finds() {
    for flag in DELIVERY_FLAGS {
        let fixture = DeliveryFixture::new();
        let homebrew = fixture.root.path("opt-homebrew-bin");
        fs::create_dir_all(&homebrew).assert_value_with("create Homebrew-style directory");
        fixture.write_fake_gh(&homebrew.join("gh"));
        let search_path = search_path(&[
            fixture.bin.clone(),
            homebrew.clone(),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ]);
        assert_eq!(
            find_on(&search_path, "gh"),
            Some(homebrew.join("gh")),
            "precondition: the user's PATH must resolve gh to the Homebrew-style directory"
        );

        let run = fixture.run(flag, &search_path).await;
        let report = fixture.report(flag, &run);
        assert!(
            String::from_utf8_lossy(&run.stdout).contains(r#""node":"deliver""#)
                || String::from_utf8_lossy(&run.stderr).contains("Node deliver"),
            "precondition: the agents must accept the change and start delivery\n{report}"
        );
        let invocation = fs::read_to_string(&fixture.gh_log).ok();
        assert!(
            invocation
                .as_deref()
                .is_some_and(|log| log.lines().next() == Some("arg=api")),
            "local {flag} delivery never started the gh at {} that the user's PATH finds; \
             delivery must resolve gh through the captured PATH\n{report}",
            homebrew.join("gh").display()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn local_delivery_without_gh_on_the_users_path_is_rejected_before_agents_start() {
    for flag in DELIVERY_FLAGS {
        let fixture = DeliveryFixture::new();
        // Only the fake harness and the host tools this run needs, so no system gh is reachable.
        let tools = fixture.root.path("tools");
        fs::create_dir_all(&tools).assert_value_with("create host tool directory");
        for tool in ["git", "cat", "grep"] {
            std::os::unix::fs::symlink(host_executable(tool), tools.join(tool))
                .assert_value_with("link host tool");
        }
        let search_path = search_path(&[fixture.bin.clone(), tools]);
        assert_eq!(
            find_on(&search_path, "gh"),
            None,
            "precondition: the user's PATH must not contain gh"
        );

        let run = fixture.run(flag, &search_path).await;
        let report = fixture.report(flag, &run);
        assert!(
            !fixture.codex_log.exists(),
            "local {flag} run was admitted and its agents ran although the user's PATH has no gh, \
             so the change could never be delivered; admission must reject it first\n{report}"
        );
        assert!(
            !run.status.success(),
            "local {flag} run without gh must fail\n{report}"
        );
        assert!(
            String::from_utf8_lossy(&run.stderr)
                .split(|character: char| !character.is_ascii_alphanumeric())
                .any(|word| word == "gh"),
            "the local {flag} rejection must name the missing gh executable\n{report}"
        );
    }
}

struct DeliveryFixture {
    root: TemporaryDirectory,
    bin: PathBuf,
    repository: PathBuf,
    state: PathBuf,
    codex_log: PathBuf,
    gh_log: PathBuf,
}

impl DeliveryFixture {
    fn new() -> Self {
        let root = TemporaryDirectory::for_test("zv2gh");
        assert!(
            !root.as_path().to_string_lossy().contains('\''),
            "fixture paths must not need shell quoting"
        );
        let bin = root.path("bin");
        let repository = root.path("repository");
        for directory in [&bin, &repository, &root.path("home")] {
            fs::create_dir_all(directory).assert_value_with("create fixture directory");
        }
        fs::write(
            root.path("config-blocker"),
            b"local commands must not open target state\n",
        )
        .assert_value_with("write config blocker");
        fs::write(root.path("input.json"), br#"{"task":"Add change.txt"}"#)
            .assert_value_with("write run input");
        fs::write(
            root.path("runtime.json"),
            // The delivery stand-in changes files on every invocation; this test isolates gh lookup.
            br#"{"harness":"codex","provider":"openai","model":"gpt-5.6","effort":"high","activeMemory":{"repository":false,"graph":false},"connections":{"openai":["OPENAI_API_KEY"]}}"#,
        )
        .assert_value_with("write uniform runtime");
        let fixture = Self {
            bin,
            state: root.path("state"),
            codex_log: root.path("codex.log"),
            gh_log: root.path("gh.log"),
            repository,
            root,
        };
        fixture.write_fake_codex();
        fixture.write_fake_git();
        fixture.initialize_repository();
        fixture
    }

    async fn run(&self, flag: &str, search_path: &OsString) -> Output {
        let path = |name: &str| self.root.path(name);
        let mut command = Command::new(env!("CARGO_BIN_EXE_zeroshot"));
        command
            .args(["run", "--title", "Local delivery gh lookup", "--template"])
            .args(["software-change", flag, "--input"])
            .arg(path("input.json"))
            .arg("--uniform-runtime-config")
            .arg(path("runtime.json"))
            .current_dir(&self.repository)
            .env_clear()
            .env("PATH", search_path)
            .env("HOME", path("home"))
            .env("ZEROSHOT_STATE_DIR", &self.state)
            .env("ZEROSHOT_CONFIG_DIR", path("config-blocker"))
            .env("OPENAI_API_KEY", "fixture-openai-key")
            .env("GH_TOKEN", "fixture-github-token")
            .kill_on_drop(true);
        timeout(CLI_TIMEOUT, command.output())
            .await
            .assert_value_with("local run deadline")
            .assert_value_with("start local run")
    }

    fn report(&self, flag: &str, run: &Output) -> String {
        let read =
            |path: &Path| fs::read_to_string(path).unwrap_or_else(|_| "<never started>".to_owned());
        format!(
            "--- zeroshot run --template software-change {flag} exit ---\n{}\n\
             --- stderr ---\n{}\n--- stdout ---\n{}\n--- fake gh invocations ---\n{}\n\
             --- fake codex turns ---\n{}",
            run.status,
            String::from_utf8_lossy(&run.stderr),
            String::from_utf8_lossy(&run.stdout),
            read(&self.gh_log),
            read(&self.codex_log)
        )
    }
    fn write_fake_git(&self) {
        let real_git = host_executable("git");

        openengine_cluster_testkit::fixture::write_executable(
            &self.bin.join("git"),
            format!(
                r#"#!/bin/sh
    is_ls_remote=false
    requested_ref=

    for argument in "$@"; do
        if [ "$argument" = "ls-remote" ]; then
            is_ls_remote=true
        fi
        case "$argument" in
            refs/heads/*) requested_ref="$argument" ;;
        esac
    done

    if [ "$is_ls_remote" = true ] && [ -n "$requested_ref" ]; then
        printf '%s\t%s\n' '0123456789abcdef0123456789abcdef01234567' "$requested_ref"
        exit 0
    fi

    exec '{real_git}' "$@"
    "#,
                real_git = real_git.display(),
            ),
            0o755,
        )
        .assert_value_with("write fake Git");
    }

    /// A GitHub CLI stand-in. Delivery clears its environment, so it logs to a fixed path.
    fn write_fake_gh(&self, path: &Path) {
        let log = self.gh_log.display();
        openengine_cluster_testkit::fixture::write_executable(
            path,
            format!(
                r#"#!/bin/sh
for argument in "$@"; do printf 'arg=%s\n' "$argument"; done >> '{log}'
printf '%s\n' '---' >> '{log}'
printf '%s\n' 'fixture gh returns no GitHub data'
"#
            ),
            0o755,
        )
        .assert_value_with("write fake gh");
    }

    /// A Codex stand-in that changes the workspace and accepts every review it is asked for.
    fn write_fake_codex(&self) {
        let log = self.codex_log.display();
        let message = |response: Value| {
            let text = json!({ "response": response }).to_string();
            json!({"type":"item.completed","item":{"type":"agent_message","text":text}}).to_string()
        };
        let worker = message(Value::Null);
        let reviewer = message(json!({
            "output": null,
            "signals": {"verdict": "accepted"},
            "diagnostic": {"message": "accepted"}
        }));
        // With delivery, the acceptance verifier also writes the pull request title and description.
        let titled_reviewer = message(json!({
            "output": {"title": "Add change.txt", "description": "Adds change.txt."},
            "signals": {"verdict": "accepted"},
            "diagnostic": {"message": "accepted"}
        }));
        openengine_cluster_testkit::fixture::write_executable(
            &self.bin.join("codex"),
            format!(
                r#"#!/bin/sh
if test "${{1-}}" = app-server; then
  while IFS= read -r request; do
    case "$request" in
      *'"method":"initialize"'*) printf '%s\n' '{{"id":1,"result":{{}}}}' ;;
      *'"method":"config/read"'*) printf '%s\n' '{{"id":2,"result":{{"config":{{"sandbox_mode":null,"approval_policy":null,"permissions":null,"active_permissions":null,"sandbox_workspace_write":null,"projects":null}},"origins":{{}},"layers":[]}}}}' ;;
      *'"method":"configRequirements/read"'*) printf '%s\n' '{{"id":3,"result":{{"requirements":null}}}}' ;;
    esac
  done
  exit 0
fi
schema=
previous=
for argument in "$@"; do
  test "$previous" = --output-schema && schema="$argument"
  previous="$argument"
done
cat >/dev/null
printf 'turn %s\n' "$schema" >> '{log}'
printf '%s\n' '{{"type":"thread.started","thread_id":"local-thread"}}'
printf '%s\n' '{{"type":"turn.started"}}'
if grep -q '"title"' "$schema"; then
  printf '%s\n' '{titled_reviewer}'
elif grep -q '"signals"' "$schema"; then
  printf '%s\n' '{reviewer}'
else
  printf 'changed\n' > "$PWD/change.txt"
  printf '%s\n' '{worker}'
fi
printf '%s\n' '{{"type":"turn.completed"}}'
"#
            ),
            0o755,
        )
        .assert_value_with("write fake Codex");
    }

    fn initialize_repository(&self) {
        let repository = &self.repository;
        git(repository, &["init"]);
        git(repository, &["config", "user.name", "Local Delivery Test"]);
        git(
            repository,
            &["config", "user.email", "local-delivery@example.invalid"],
        );
        git(repository, &["config", "commit.gpgsign", "false"]);
        fs::write(repository.join("seed.txt"), b"seed\n").assert_value_with("seed repository");
        git(repository, &["add", "seed.txt"]);
        git(repository, &["commit", "-m", "seed"]);
        git(repository, &["branch", "-M", "feature/local"]);
        git(
            repository,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/local-fixture.git",
            ],
        );
    }
}

impl Drop for DeliveryFixture {
    fn drop(&mut self) {
        let Ok(runs) = fs::read_dir(self.state.join("runs")) else {
            return;
        };
        for ready in runs
            .flatten()
            .map(|entry| entry.path().join("controller.ready.json"))
        {
            let pid = fs::read(ready)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|ready| ready.get("pid").and_then(Value::as_u64))
                .and_then(|pid| i32::try_from(pid).ok());
            if let Some(pid) = pid {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
    }
}

fn search_path(directories: &[PathBuf]) -> OsString {
    std::env::join_paths(directories).assert_value_with("join search path")
}

fn find_on(search_path: &OsString, program: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;

    std::env::split_paths(search_path)
        .map(|directory| directory.join(program))
        .find(|candidate| {
            fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

fn host_executable(program: &str) -> PathBuf {
    let search_path = std::env::var_os("PATH").assert_value_with("test process PATH");
    find_on(&search_path, program).assert_value_with("host tool on the test process PATH")
}

fn git(repository: &Path, arguments: &[&str]) {
    let output = StdCommand::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .output()
        .assert_value_with("run Git fixture command");
    assert!(
        output.status.success(),
        "Git fixture command {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
