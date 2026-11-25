#![cfg(unix)]

//! Local delivery captures the caller's Git and native HTTPS settings.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use openengine_cluster_testkit::TemporaryDirectory;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};
use reqwest::{Method, StatusCode};
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::timeout;

#[allow(dead_code)]
#[path = "../src/source_code/adapters/core/tests/http_fixture.rs"]
mod http_fixture;
use http_fixture::{IntoResponse, Json, Request, Response};

const TOKEN: &str = "fixture-github-token";
const REJECTED_TOKEN: &str = "rejected-checkpoint-token";
const PROXY: &str = "http://caller:checkpoint-proxy-secret@127.0.0.1:9";
const NO_PROXY: &str = "127.0.0.1,caller-bypass.invalid";
const CLI_TIMEOUT: Duration = Duration::from_secs(90);
const DELIVERY_FLAGS: [&str; 2] = ["--pr", "--ship"];

#[path = "native_v2_local_delivery_gh_search_path/checkpoint_tests.rs"]
mod checkpoint_tests;

#[tokio::test(flavor = "multi_thread")]
async fn local_delivery_starts_the_git_that_the_users_path_finds() {
    for flag in DELIVERY_FLAGS {
        let fixture = DeliveryFixture::new().await;
        let homebrew = fixture.root.path("opt-homebrew-bin");
        fs::create_dir_all(&homebrew).assert_value_with("create Homebrew-style directory");
        fs::rename(fixture.bin.join("git"), homebrew.join("git")).assert_value();
        openengine_cluster_testkit::fixture::write_executable(
            &homebrew.join("gh"),
            format!(
                "#!/bin/sh
printf '%s\n' used > '{}'
exit 1
",
                fixture.root.path("gh.log").display()
            ),
            0o755,
        )
        .assert_value();
        let search_path = search_path(&[
            fixture.bin.clone(),
            homebrew.clone(),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ]);
        assert_eq!(find_on(&search_path, "git"), Some(homebrew.join("git")));

        let run = fixture.run(flag, &search_path).await;
        fixture.assert_delivery_started(flag, &run);
        let invocation = fs::read_to_string(&fixture.git_log).assert_value();
        assert!(
            invocation
                .lines()
                .any(|line| Path::new(line) == homebrew.join("git"))
        );
        assert!(!fixture.root.path("gh.log").exists());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn local_delivery_without_gh_on_the_users_path_reaches_native_forge() {
    for flag in DELIVERY_FLAGS {
        let fixture = DeliveryFixture::new().await;
        let tools = fixture.link_host_tools();
        let search_path = search_path(&[fixture.bin.clone(), tools]);
        assert_eq!(find_on(&search_path, "gh"), None);

        let run = fixture.run(flag, &search_path).await;
        fixture.assert_delivery_started(flag, &run);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn local_delivery_rejects_preexisting_changes_before_agents_start() {
    for flag in ["--push", "--pr", "--ship"] {
        let fixture = DeliveryFixture::new().await;
        git(
            &fixture.repository,
            &["config", "core.excludesFile", "../excludes"],
        );
        fs::write(fixture.root.path("excludes"), "ignored.log\n").assert_value();
        fs::write(fixture.repository.join("seed.txt"), "edited\n").assert_value();
        fs::write(fixture.repository.join("staged.txt"), "staged\n").assert_value();
        git(&fixture.repository, &["add", "staged.txt"]);
        fs::write(fixture.repository.join("my-notes.txt"), "private\n").assert_value();
        fs::write(fixture.repository.join("ignored.log"), "ignored\n").assert_value();
        let search_path = search_path(&[
            fixture.bin.clone(),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ]);

        let run = fixture
            .run_arguments(&[flag, "--detach"], &search_path)
            .await;
        let error = String::from_utf8_lossy(&run.stderr);
        assert!(!run.status.success(), "dirty {flag} was admitted: {run:?}");
        assert!(!fixture.codex_log.exists(), "agents ran before refusal");
        for path in ["seed.txt", "staged.txt", "my-notes.txt"] {
            assert!(error.contains(path), "dirty path {path} missing: {error}");
        }
        assert!(
            !error.contains("ignored.log"),
            "ignored path was reported: {error}"
        );

        let no_delivery = fixture
            .run_arguments(&["--delivery", "none", "--detach"], &search_path)
            .await;
        assert!(no_delivery.status.success(), "{no_delivery:?}");
    }
}

struct DeliveryFixture {
    root: TemporaryDirectory,
    bin: PathBuf,
    repository: PathBuf,
    state: PathBuf,
    codex_log: PathBuf,
    git_log: PathBuf,
    forge_host: String,
    root_ca: PathBuf,
    ca_file: PathBuf,
    forge: Arc<Forge>,
    server: JoinHandle<()>,
}

impl DeliveryFixture {
    async fn new() -> Self {
        let root = TemporaryDirectory::for_test("zv2forge");
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
            br#"{"harness":"codex","provider":"openai","model":"gpt-5.6","effort":"high","activeMemory":{"repository":false,"graph":false},"connections":{"openai":["OPENAI_API_KEY"]}}"#,
        )
        .assert_value_with("write uniform runtime");
        let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
        let forge_host = listener.local_addr().assert_value().to_string();
        let root_ca = root.path("forge-ca.pem");
        let tls = http_fixture::loopback_tls(&root_ca);
        let forge = Arc::new(Forge::default());
        let server = tokio::spawn(http_fixture::serve_tls(listener, tls, {
            let forge = forge.clone();
            move |request| forge.reply(request)
        }));
        let fixture = Self {
            bin,
            state: root.path("state"),
            codex_log: root.path("codex.log"),
            git_log: root.path("git.log"),
            repository,
            forge_host,
            ca_file: root_ca.clone(),
            root_ca,
            forge,
            server,
            root,
        };
        fixture.write_fake_codex();
        fixture.write_fake_git();
        fixture.initialize_repository();
        fixture
    }

    async fn run(&self, flag: &str, search_path: &OsString) -> Output {
        self.run_arguments(&[flag], search_path).await
    }

    async fn run_arguments(&self, flags: &[&str], search_path: &OsString) -> Output {
        let path = |name: &str| self.root.path(name);
        let mut command = self.command(search_path);
        command
            .args(["run", "--title", "Local delivery HTTPS", "--template"])
            .arg("software-change")
            .args(flags)
            .arg("--input")
            .arg(path("input.json"))
            .arg("--uniform-runtime-config")
            .arg(path("runtime.json"));
        if flags
            .iter()
            .any(|flag| matches!(*flag, "--push" | "--pr" | "--ship"))
        {
            command.args([
                "--source-provider",
                "source.github",
                "--source-base-url",
                &format!("https://{}", self.forge_host),
            ]);
        }
        timeout(CLI_TIMEOUT, command.output())
            .await
            .assert_value_with("local run deadline")
            .assert_value_with("start local run")
    }

    fn command(&self, search_path: &OsString) -> Command {
        let path = |name: &str| self.root.path(name);
        let mut command = Command::new(env!("CARGO_BIN_EXE_zeroshot"));
        command
            .current_dir(&self.repository)
            .env_clear()
            .env("PATH", search_path)
            .env("HOME", path("home"))
            .env("ZEROSHOT_STATE_DIR", &self.state)
            .env("ZEROSHOT_CONFIG_DIR", path("config-blocker"))
            .env("OPENAI_API_KEY", "fixture-openai-key")
            .env("GH_HOST", &self.forge_host)
            .env("GH_ENTERPRISE_TOKEN", TOKEN)
            .env("GH_TOKEN", TOKEN)
            .env("SSL_CERT_FILE", &self.ca_file)
            .kill_on_drop(true);
        if let Some(restic) = std::env::var_os("ZEROSHOT_RESTIC") {
            command.env("ZEROSHOT_RESTIC", restic);
        }
        command
    }

    fn assert_delivery_started(&self, flag: &str, run: &Output) {
        let report = format!(
            "delivery {flag} exit {}\n--- stderr ---\n{}\n--- stdout ---\n{}",
            run.status,
            String::from_utf8_lossy(&run.stderr),
            String::from_utf8_lossy(&run.stdout),
        );
        assert!(self.codex_log.exists(), "{report}");
        assert_eq!(
            self.forge.project_reads.load(Ordering::SeqCst),
            1,
            "{report}"
        );
        assert!(
            String::from_utf8_lossy(&run.stdout).contains(r#""node":"deliver""#)
                || String::from_utf8_lossy(&run.stderr).contains("Node deliver"),
            "{report}",
        );
    }

    fn link_host_tools(&self) -> PathBuf {
        let tools = self.root.path("host-tools");
        fs::create_dir(&tools).assert_value();
        let cleanup = cfg!(not(target_os = "linux")).then_some("ps");
        for program in ["cat", "grep"].into_iter().chain(cleanup) {
            std::os::unix::fs::symlink(host_executable(program), tools.join(program))
                .assert_value();
        }
        tools
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
        printf '%s\n' "$0" >> '{log}'
        printf '%s\t%s\n' '0123456789abcdef0123456789abcdef01234567' "$requested_ref"
        exit 0
    fi

    exec '{real_git}' "$@"
    "#,
                real_git = real_git.display(),
                log = self.git_log.display(),
            ),
            0o755,
        )
        .assert_value_with("write fake Git");
    }

    /// A Codex stand-in that changes the workspace and accepts every review it is asked for.
    fn write_fake_codex(&self) {
        let log = self.codex_log.display();
        let message = |response: Value| {
            let text = json!({ "response": response }).to_string();
            json!({"type":"item.completed","item":{"type":"agent_message","text":text}}).to_string()
        };
        let worker =
            message(json!({"reviewResponses": [], "reviewContext": "Prepared change.txt."}));
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
if [ "$*" = --version ]; then printf 'codex-cli 0.155.0\n'; exit 0; fi
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
        fs::write(repository.join(".git/info/exclude"), "*.profraw\n")
            .assert_value_with("exclude coverage profiles from the delivery fixture");
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
            &["config", "branch.feature/local.remote", "origin"],
        );
        git(
            repository,
            &["config", "branch.feature/local.merge", "refs/heads/main"],
        );
        git(
            repository,
            &[
                "remote",
                "add",
                "origin",
                &format!("https://{}/acme/local-fixture.git", self.forge_host),
            ],
        );
    }
}

impl Drop for DeliveryFixture {
    fn drop(&mut self) {
        self.server.abort();
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

#[derive(Default)]
struct Forge {
    project_reads: AtomicUsize,
    stall: AtomicBool,
    disconnected: Arc<Notify>,
}

impl Forge {
    fn reply(&self, request: Request) -> Response {
        if request.method != Method::GET || request.uri.path() != "/api/v3/repos/acme/local-fixture"
        {
            return StatusCode::NOT_FOUND.into_response();
        }
        self.project_reads.fetch_add(1, Ordering::SeqCst);
        if self.stall.load(Ordering::SeqCst) {
            return Response::stall_until_disconnect(self.disconnected.clone());
        }
        if request
            .headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            != Some(&format!("Bearer {TOKEN}"))
        {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({"message": format!("rejected {REJECTED_TOKEN} at {PROXY}")})),
            )
                .into_response();
        }
        Json(json!({"full_name":"acme/local-fixture"}))
            .into_response()
            .header("x-github-enterprise-version", "enterprise-server@3.22.2")
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
