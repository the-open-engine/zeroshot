#![cfg(unix)]

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::{self as acp, Agent as _};
use async_trait::async_trait;
use fs2::FileExt as _;
use serde_json::{json, Value};
#[cfg(feature = "ui")]
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};

#[derive(Default)]
struct RecordingClient {
    messages: Mutex<Vec<String>>,
}

#[async_trait(?Send)]
impl acp::Client for RecordingClient {
    async fn request_permission(
        &self,
        _request: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        Err(acp::Error::method_not_found())
    }

    async fn session_notification(
        &self,
        notification: acp::SessionNotification,
    ) -> acp::Result<()> {
        if let acp::SessionUpdate::AgentMessageChunk(chunk) = notification.update {
            if let acp::ContentBlock::Text(text) = chunk.content {
                self.messages.lock().unwrap().push(text.text);
            }
        }
        Ok(())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn acp_turns_reuse_provider_session_and_remain_observable() {
    let fixture = AcpFixture::new();
    fixture.install_profile();
    let (run_ids, messages) = fixture.run_session().await;

    assert_eq!(messages, ["first", "second"]);
    assert_eq!(run_ids.len(), 2);
    assert_ne!(run_ids[0], run_ids[1]);
    let capture = std::fs::read_to_string(&fixture.capture).unwrap();
    assert_eq!(capture.matches("arg=resume\n").count(), 1);
    assert_eq!(capture.matches("arg=acp-test-thread\n").count(), 1);
    assert_eq!(capture.matches("environment=prepared\n").count(), 2);
    fixture.assert_durable_runs(&run_ids);
}

#[derive(Clone, Copy, Debug)]
enum Interruption {
    Cancel,
    LoseWorkspace,
}

#[tokio::test(flavor = "current_thread")]
async fn active_turn_interruption_is_terminal_and_bounded() {
    for interruption in [Interruption::Cancel, Interruption::LoseWorkspace] {
        let fixture = AcpFixture::new();
        fixture.install_profile();
        let started = fixture._root.path().join("provider-started");
        let mut child = fixture.spawn_stalled_server();
        let outgoing = child.stdin.take().unwrap();
        let incoming = child.stdout.take().unwrap();
        let workspace = fixture.workspace.clone();
        let state = fixture.state.clone();

        tokio::task::LocalSet::new()
            .run_until(async move {
                let (connection, io, session_id) = open_session(
                    outgoing,
                    incoming,
                    workspace,
                    Arc::new(RecordingClient::default()),
                )
                .await;
                let mut prompt = Box::pin(connection.prompt(acp::PromptRequest::new(
                    session_id.clone(),
                    vec!["interrupt me".into()],
                )));
                tokio::select! {
                    () = wait_for_file(&started) => {}
                    result = &mut prompt => {
                        panic!("prompt finished before {interruption:?}: {result:?}")
                    }
                }

                let competing_lease = match interruption {
                    Interruption::Cancel => {
                        connection
                            .cancel(acp::CancelNotification::new(session_id.clone()))
                            .await
                            .unwrap();
                        None
                    }
                    Interruption::LoseWorkspace => {
                        let lease = only_file(&state.join("workspaces"));
                        std::fs::remove_file(&lease).unwrap();
                        let replacement = OpenOptions::new()
                            .create_new(true)
                            .read(true)
                            .write(true)
                            .open(&lease)
                            .unwrap();
                        replacement.try_lock_exclusive().unwrap();
                        Some(replacement)
                    }
                };

                let response = tokio::time::timeout(Duration::from_secs(5), &mut prompt)
                    .await
                    .expect("interrupted prompt did not finish")
                    .unwrap();
                let (stop_reason, failure) = match interruption {
                    Interruption::Cancel => (acp::StopReason::Cancelled, "force_stopped"),
                    Interruption::LoseWorkspace => (acp::StopReason::EndTurn, "runtime_lost"),
                };
                assert_eq!(response.stop_reason, stop_reason);
                assert_eq!(
                    response.meta.unwrap()["zeroshot"]["rawOutput"]["failed"],
                    failure
                );
                drop(prompt);
                if matches!(interruption, Interruption::LoseWorkspace) {
                    assert!(
                        connection
                            .prompt(acp::PromptRequest::new(
                                session_id.clone(),
                                vec!["must be rejected".into()],
                            ))
                            .await
                            .is_err()
                    );
                }
                connection
                    .close_session(acp::CloseSessionRequest::new(session_id))
                    .await
                    .unwrap();
                drop(competing_lease);
                drop(connection);
                disconnect_client(io).await;
            })
            .await;
        assert_server_exits(&mut child).await;
    }
}

#[cfg(feature = "ui")]
#[tokio::test(flavor = "current_thread")]
async fn active_acp_turn_streams_through_the_workspace_ui() {
    let fixture = AcpFixture::new();
    fixture.install_profile();
    let mut acp_server = fixture.spawn_stalled_server();
    let outgoing = acp_server.stdin.take().unwrap();
    let incoming = acp_server.stdout.take().unwrap();
    let mut ui_server = fixture.spawn_ui();
    let origin = read_ui_origin(&mut ui_server).await;
    let workspace = fixture.workspace.clone();
    let state = fixture.state.clone();
    let started = fixture._root.path().join("provider-started");
    let release = fixture._root.path().join("provider-release");

    tokio::task::LocalSet::new()
        .run_until(async move {
            let client = Arc::new(RecordingClient::default());
            let (connection, io, session_id) =
                open_session(outgoing, incoming, workspace, client).await;
            let request = acp::PromptRequest::new(session_id.clone(), vec!["show this run".into()]);
            let mut prompt = Box::pin(connection.prompt(request));
            tokio::select! {
                () = wait_for_file(&started) => {}
                result = &mut prompt => panic!("prompt finished before UI inspection: {result:?}"),
            }
            let run_id = only_file(&state.join("runs"))
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            let (client, events, mut body) = open_live_ui_run(&origin, &run_id).await;

            std::fs::write(release, []).unwrap();
            let response = tokio::time::timeout(Duration::from_secs(5), &mut prompt)
                .await
                .expect("ACP prompt did not finish")
                .unwrap();
            assert_eq!(response.stop_reason, acp::StopReason::EndTurn);
            body.push_str(
                &tokio::time::timeout(Duration::from_secs(5), events.text())
                    .await
                    .expect("UI live history did not close after the terminal event")
                    .unwrap(),
            );
            assert!(body.contains("\"kind\":\"terminal\""));
            assert!(!body.contains("event: history_error\n"));

            let finished: Value = client
                .get(format!("{origin}/ui/api/runs/{run_id}"))
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_eq!(finished["phase"], "finished");
            assert_eq!(finished["terminal"]["status"], "succeeded");
            connection
                .close_session(acp::CloseSessionRequest::new(session_id))
                .await
                .unwrap();
            drop(prompt);
            drop(connection);
            disconnect_client(io).await;
        })
        .await;
    assert_server_exits(&mut acp_server).await;
    ui_server.start_kill().unwrap();
    ui_server.wait().await.unwrap();
}

struct AcpFixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    config: PathBuf,
    state: PathBuf,
    capture: PathBuf,
    executable: PathBuf,
    path: String,
}

impl AcpFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        let config = root.path().join("config");
        let state = root.path().join("state");
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        initialize_workspace(&workspace);

        let capture = root.path().join("codex-invocations");
        let provider_state = root.path().join("codex-state");
        install_fake_codex(&bin, &capture, &provider_state);
        let executable = PathBuf::from(env!("CARGO_BIN_EXE_zeroshot"));
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        Self {
            _root: root,
            workspace,
            config,
            state,
            capture,
            executable,
            path,
        }
    }

    fn environment(&self) -> [(&'static str, &OsStr); 3] {
        [
            ("ZEROSHOT_CONFIG_DIR", self.config.as_os_str()),
            ("ZEROSHOT_STATE_DIR", self.state.as_os_str()),
            ("PATH", self.path.as_ref()),
        ]
    }

    fn install_profile(&self) {
        // Seed the same authoring-store resource shape used by the environment API.
        let resources = json!({"environments":{"acp-public":{
            "id":"acp-public", "name":"acp-public", "revision":"fixture-revision",
            "definition":{"variables":{"ACP_PUBLIC_SETTING":"prepared"}}
        }}});
        std::fs::write(
            self.config.join("profiles.json"),
            serde_json::to_vec(&resources).unwrap(),
        )
        .unwrap();
        let (graph, runtime) = write_profile_files(self._root.path());
        let profile = Command::new(&self.executable)
            .args([
                "profile",
                "set",
                "acp-test",
                "--graph",
                graph.to_str().unwrap(),
                "--runtime-config",
                runtime.to_str().unwrap(),
            ])
            .current_dir(&self.workspace)
            .envs(self.environment())
            .output()
            .unwrap();
        assert!(
            profile.status.success(),
            "profile setup failed: {}",
            String::from_utf8_lossy(&profile.stderr)
        );
    }

    fn spawn_server(&self) -> tokio::process::Child {
        let mut command = tokio::process::Command::new(&self.executable);
        command
            .args(["acp", "--profile", "local:acp-test"])
            .current_dir(&self.workspace)
            .envs(self.environment())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command.spawn().unwrap()
    }

    fn spawn_stalled_server(&self) -> tokio::process::Child {
        std::fs::write(self._root.path().join("provider-gate-enabled"), []).unwrap();
        self.spawn_server()
    }

    #[cfg(feature = "ui")]
    fn spawn_ui(&self) -> tokio::process::Child {
        let mut command = tokio::process::Command::new(&self.executable);
        command
            .args(["ui", "--listen", "127.0.0.1:0"])
            .current_dir(&self.workspace)
            .envs(self.environment())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        command.spawn().unwrap()
    }

    async fn run_session(&self) -> (Vec<String>, Vec<String>) {
        let mut child = self.spawn_server();
        let outgoing = child.stdin.take().unwrap();
        let incoming = child.stdout.take().unwrap();
        let client = Arc::new(RecordingClient::default());
        let session_workspace = self.workspace.clone();
        let result = tokio::task::LocalSet::new()
            .run_until(async move {
                let (connection, io, session_id) = open_session(
                    outgoing,
                    incoming,
                    session_workspace.clone(),
                    client.clone(),
                )
                .await;
                connection
                    .cancel(acp::CancelNotification::new(session_id.clone()))
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(20)).await;
                let mut run_ids = Vec::new();
                for (index, (task, expected)) in
                    [("first task", "first"), ("second task", "second")]
                        .into_iter()
                        .enumerate()
                {
                    let response = connection
                        .prompt(acp::PromptRequest::new(
                            session_id.clone(),
                            vec![task.into()],
                        ))
                        .await
                        .unwrap();
                    assert_eq!(response.stop_reason, acp::StopReason::EndTurn);
                    let meta = response.meta.unwrap();
                    let zeroshot = meta.get("zeroshot").unwrap();
                    run_ids.push(zeroshot["runId"].as_str().unwrap().to_owned());
                    assert_eq!(zeroshot["rawOutput"]["response"], expected);
                    if index == 0 {
                        std::fs::write(session_workspace.join("turn.txt"), "second revision\n")
                            .unwrap();
                        run(&session_workspace, "git", &["add", "turn.txt"]);
                        run(
                            &session_workspace,
                            "git",
                            &["commit", "-m", "test: advance source"],
                        );
                    }
                }
                drop(connection);
                disconnect_client(io).await;
                let messages = client.messages.lock().unwrap().clone();
                (run_ids, messages)
            })
            .await;
        assert_server_exits(&mut child).await;
        result
    }

    fn assert_durable_runs(&self, run_ids: &[String]) {
        let mut revisions = Vec::new();
        for (run_id, expected) in run_ids.iter().zip(["first", "second"]) {
            let output = Command::new(&self.executable)
                .args(["status", run_id])
                .current_dir(&self.workspace)
                .envs(self.environment())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "status failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let status: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                status["status"]["terminalResult"]["output"]["response"],
                expected
            );
            revisions.push(status["source"]["revision"].as_str().unwrap().to_owned());
            assert!(
                self.state
                    .join("runs")
                    .join(run_id)
                    .join("runs.sqlite3")
                    .is_file()
            );
        }
        assert_ne!(revisions[0], revisions[1]);
    }
}

async fn open_session(
    outgoing: tokio::process::ChildStdin,
    incoming: tokio::process::ChildStdout,
    workspace: PathBuf,
    client: Arc<RecordingClient>,
) -> (
    acp::ClientSideConnection,
    tokio::task::JoinHandle<acp::Result<()>>,
    acp::SessionId,
) {
    let (connection, io) = acp::ClientSideConnection::new(
        client,
        outgoing.compat_write(),
        incoming.compat(),
        |future| {
            tokio::task::spawn_local(future);
        },
    );
    let io = tokio::task::spawn_local(io);
    let initialized = connection
        .initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
        .await
        .unwrap();
    assert_eq!(initialized.protocol_version, acp::ProtocolVersion::V1);
    let session = connection
        .new_session(acp::NewSessionRequest::new(workspace))
        .await
        .unwrap();
    (connection, io, session.session_id)
}

#[cfg(feature = "ui")]
async fn read_ui_origin(child: &mut tokio::process::Child) -> String {
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(line) = lines.next_line().await.unwrap() {
            if let Some(url) = line.strip_prefix("Zeroshot UI: ") {
                return url.trim_end_matches("/ui/").to_owned();
            }
        }
        panic!("UI exited before publishing its address");
    })
    .await
    .expect("UI did not publish its address")
}

#[cfg(feature = "ui")]
async fn open_live_ui_run(
    origin: &str,
    run_id: &str,
) -> (reqwest::Client, reqwest::Response, String) {
    let client = reqwest::Client::new();
    let listed: Value = client
        .get(format!("{origin}/ui/api/runs"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(listed["runs"][0]["runId"], run_id);
    assert_eq!(listed["runs"][0]["phase"], "running");
    let detail: Value = client
        .get(format!("{origin}/ui/api/runs/{run_id}"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail["title"], "ACP turn");
    assert_eq!(detail["graph"]["root"]["name"], "root");

    let mut events = client
        .get(format!("{origin}/ui/api/runs/{run_id}/events?after=v2%3A0"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let body = tokio::time::timeout(Duration::from_secs(2), async {
        let mut body = String::new();
        while !body.contains("event: history\n") && !body.contains("event: history_error\n") {
            let chunk = events
                .chunk()
                .await
                .unwrap()
                .expect("UI closed the live history stream");
            body.push_str(std::str::from_utf8(&chunk).unwrap());
        }
        body
    })
    .await
    .expect("UI did not send the initial live history page");
    assert!(body.contains("event: history\n"));
    assert!(!body.contains("event: history_error\n"));
    (client, events, body)
}

async fn wait_for_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !path.is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {}", path.display()));
}

fn only_file(directory: &Path) -> PathBuf {
    let files = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(
        files.len(),
        1,
        "expected one file in {}",
        directory.display()
    );
    files.into_iter().next().unwrap()
}

async fn disconnect_client(io: tokio::task::JoinHandle<acp::Result<()>>) {
    io.abort();
    assert!(io.await.unwrap_err().is_cancelled());
}

async fn assert_server_exits(child: &mut tokio::process::Child) {
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("ACP server did not exit after stdin EOF")
        .unwrap();
    assert!(status.success());
}

fn initialize_workspace(workspace: &Path) {
    run(workspace, "git", &["init", "--initial-branch=main"]);
    run(workspace, "git", &["config", "user.name", "ACP Test"]);
    run(
        workspace,
        "git",
        &["config", "user.email", "acp@example.invalid"],
    );
    std::fs::write(workspace.join("README.md"), "fixture\n").unwrap();
    run(workspace, "git", &["add", "README.md"]);
    run(workspace, "git", &["commit", "-m", "test: initialize"]);
    run(
        workspace,
        "git",
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/open-engine/zeroshot.git",
        ],
    );
}

fn run(directory: &Path, program: &str, arguments: &[&str]) {
    let status = Command::new(program)
        .args(arguments)
        .current_dir(directory)
        .status()
        .unwrap();
    assert!(status.success(), "{program} {arguments:?} failed");
}

fn install_fake_codex(bin: &Path, capture: &Path, state: &Path) {
    let root = state.parent().unwrap();
    let gate = root.join("provider-gate-enabled");
    let started = root.join("provider-started");
    let release = root.join("provider-release");
    let script = format!(
        r#"#!/bin/sh
set -eu
if [ "${{1-}}" = app-server ]; then
  exit 1
fi
/usr/bin/cat >/dev/null
if [ -e {gate:?} ]; then
  : > {started:?}
  while [ ! -e {release:?} ]; do /usr/bin/sleep 0.02; done
fi
{{
  for argument in "$@"; do /usr/bin/printf 'arg=%s\n' "$argument"; done
  /usr/bin/printf 'environment=%s\n' "${{ACP_PUBLIC_SETTING-}}"
  /usr/bin/printf '%s\n' '---'
}} >> {capture:?}
/usr/bin/printf '%s\n' '{{"type":"thread.started","thread_id":"acp-test-thread"}}'
if [ -e {state:?} ]; then
  response=second
else
  : > {state:?}
  response=first
fi
/usr/bin/printf '%s%s%s%s\n' \
  '{{"type":"item.completed","item":{{"type":"agent_message",' \
  '"text":"{{\"response\":{{\"response\":\"' \
  "$response" \
  '\"}}}}"}}}}'
/usr/bin/printf '%s\n' '{{"type":"turn.completed"}}'
"#,
        capture = capture,
        state = state,
        gate = gate,
        started = started,
        release = release,
    );
    let executable = bin.join("codex");
    openengine_cluster_testkit::fixture::write_executable(&executable, script, 0o755).unwrap();
}

fn write_profile_files(root: &Path) -> (PathBuf, PathBuf) {
    let string = || json!({"kind":"string"});
    let field = || json!({"type":string(),"required":true});
    let state = json!({"kind":"record","fields":{
        "task":field(),
        "response":field()
    }});
    let graph = json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":{"kind":"record","fields":{"task":field()}},
        "policy":{"policy":"policy.native-v2@1","default":"deny"},
        "root":{
            "kind":"seq","name":"root","state":state,
            "children":[
                {
                    "kind":"step","name":"worker","worker":"agent.worker@1",
                    "instructions":"Return a concise response to the task.",
                    "input":{"kind":"record","fields":{"task":field()}},
                    "output":{"kind":"record","fields":{"response":field()}},
                    "inputBindings":[{
                        "target":["task"],
                        "value":{"source":"state","path":["task"]}
                    }],
                    "writeBindings":[{
                        "target":["response"],
                        "value":{"node":"worker","channel":"out","path":["response"]}
                    }],
                    "attempts":1
                },
                {
                    "kind":"succeed","name":"done",
                    "output":{"kind":"record","fields":{"response":field()}},
                    "bindings":[{
                        "target":["response"],
                        "value":{"source":"state","path":["response"]}
                    }]
                }
            ],
            "promotedStatePaths":[]
        }
    });
    let runtime = json!({
        "harness":"codex",
        "provider":"openai",
        "size":"medium",
        "environment":{"id":"acp-public"},
        "nodes":{"worker":{
            "kind":"agent",
            "model":"test-model",
            "sessionScope":"node_instance"
        }}
    });
    let graph_path = root.join("graph.json");
    let runtime_path = root.join("runtime.json");
    std::fs::write(&graph_path, serde_json::to_vec(&graph).unwrap()).unwrap();
    std::fs::write(&runtime_path, serde_json::to_vec(&runtime).unwrap()).unwrap();
    (graph_path, runtime_path)
}
