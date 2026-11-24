//! Native process lifecycle on Linux and Windows. A completed, a cancelled and a timed-out turn
//! leave no OpenCode process behind, and two concurrent nodes keep their own login and store.
use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

fn binary() -> String {
    std::env::var("ZEROSHOT_OPENCODE_BINARY").expect("set the explicit native OpenCode binary")
}

fn model() -> Value {
    json!({"id":"probe-model","name":"Probe","limit":{"context":100000,"output":4096}})
}

/// A user profile: XDG directories below `directory` and the given inline configuration.
fn profile(directory: &TestDirectory, config: Value) -> BTreeMap<String, String> {
    let mut environment: BTreeMap<String, String> = [
        ("XDG_DATA_HOME", "data"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_STATE_HOME", "state"),
    ]
    .into_iter()
    .map(|(name, path)| {
        (
            name.to_owned(),
            directory.child(path).to_string_lossy().into_owned(),
        )
    })
    .collect();
    environment.extend([
        ("OPENCODE_DISABLE_MODELS_FETCH".to_owned(), "1".to_owned()),
        ("OPENCODE_CONFIG_CONTENT".to_owned(), config.to_string()),
    ]);
    environment
}

/// The installed OpenCode with a profile's environment, and `home` as its home on every platform.
fn opencode(home: &Path, environment: &BTreeMap<String, String>) -> std::process::Command {
    let mut command = std::process::Command::new(binary());
    command
        .envs(environment)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .current_dir(home);
    command
}

/// A local `anthropic` node that runs the installed OpenCode directly, with `home` as the user's
/// home and `environment` as the user's OpenCode environment.
fn node(
    directory: &TestDirectory,
    home: &Path,
    environment: &BTreeMap<String, String>,
) -> OpenCodeAdapter {
    let workspace = directory.child("workspace");
    let runtime_home = directory.child("runtime");
    std::fs::create_dir_all(&workspace).assert_value();
    std::fs::create_dir_all(&runtime_home).assert_value();
    let path = std::env::var("PATH").assert_value();
    let mut adapter = OpenCodeAdapter::new_local(OpenCodeConfig {
        provider: "anthropic".to_owned(),
        executable: binary(),
        prefix_arguments: vec![],
        workspace,
        runtime_home,
        local_user_home: Some(home.to_owned()),
        native_environment: LocalHarnessEnvironment::default(),
        base_environment: OpenCodeProcessEnvironment::new(BTreeMap::from([(
            "PATH".to_owned(),
            path,
        )]))
        .assert_value(),
        process_pool: HostedProcessPool::hosted_default(),
    })
    .assert_value();
    adapter.local_environment = environment.clone();
    adapter
}

fn invocation(fields: &[(&str, &str)]) -> DriverInvocation {
    crate::harnesses::adapters::contributed::test_support::invocation(
        "opencode",
        fields,
        Arc::new(OpenCodeSession {
            core: ProviderSessionCore::new(),
            id: Mutex::new(None),
            login: Mutex::new(None),
        }),
    )
}

/// A node with a declared Anthropic key; its user profile lives in `directory`.
fn declared_node(directory: &TestDirectory) -> OpenCodeAdapter {
    let config = json!({"provider":{"anthropic":{"options":{"apiKey":"stored-key"},"models":{"opaque/model":model()}}}});
    node(directory, directory.path(), &profile(directory, config))
}

fn declared(endpoint: &str) -> DriverInvocation {
    invocation(&[
        ("ANTHROPIC_API_KEY", "declared-lifecycle-key"),
        ("ANTHROPIC_BASE_URL", endpoint),
    ])
}

async fn listener() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .assert_value();
    let endpoint = format!("http://{}/v1", listener.local_addr().assert_value());
    (listener, endpoint)
}

/// What the local provider does with every request.
#[derive(Clone, Copy)]
enum Reply {
    /// Answers with a complete turn once the gate opens.
    Answer,
    /// Never answers.
    Hold,
    /// Sends the response head and the first stream event, then stays silent.
    Stall,
}

/// A complete Anthropic message stream whose text is the node's expected response.
fn answer() -> String {
    let events = [
        json!({"type":"message_start","message":{"id":"msg_probe","type":"message","role":"assistant","model":"probe-model","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"{\"response\":null}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":1}}),
        json!({"type":"message_stop"}),
    ];
    events
        .iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

/// Reads one request (headers and a declared-length body); returns its lower-case header block.
async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    loop {
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= end + 4 + length {
                return Some(head);
            }
        }
        let mut chunk = [0; 8192];
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 || bytes.len() > 8 * 1024 * 1024 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

/// Waits until the client closes the connection, for at most two minutes.
async fn until_closed(stream: &mut tokio::net::TcpStream) {
    let _ = tokio::time::timeout(Duration::from_secs(120), async {
        let mut chunk = [0; 8192];
        while stream.read(&mut chunk).await.unwrap_or(0) > 0 {}
    })
    .await;
}

/// A local Anthropic-compatible provider; every request's header block goes to `seen`.
async fn provider(
    listener: tokio::net::TcpListener,
    reply: Reply,
    seen: mpsc::UnboundedSender<String>,
    gate: watch::Receiver<bool>,
) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let seen = seen.clone();
        let mut gate = gate.clone();
        tokio::spawn(async move {
            let Some(head) = read_request(&mut stream).await else {
                return;
            };
            let _ = seen.send(head);
            match reply {
                Reply::Answer => {
                    if gate.wait_for(|open| *open).await.is_err() {
                        return;
                    }
                    let payload = answer();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
                Reply::Hold => until_closed(&mut stream).await,
                Reply::Stall => {
                    let stream_start = answer();
                    let first = stream_start
                        .split_inclusive("\n\n")
                        .next()
                        .unwrap_or_default();
                    let started = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n{first}"
                    );
                    let _ = stream.write_all(started.as_bytes()).await;
                    let _ = stream.flush().await;
                    until_closed(&mut stream).await;
                }
            }
        });
    }
}

/// A live process: its id and its start time. The start time keeps a reused id from passing for
/// a process seen earlier.
type Process = (u32, u64);

/// Parent id and start time of every live process, and the id of a lister process to ignore.
#[cfg(target_os = "linux")]
fn process_table() -> (BTreeMap<u32, Process>, Option<u32>) {
    let mut table = BTreeMap::new();
    for entry in std::fs::read_dir("/proc").assert_value().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // The command name may contain spaces; the fields after its closing parenthesis are fixed:
        // the parent id is the second of them and the start time the twentieth.
        let Some((_, fields)) = stat.rsplit_once(')') else {
            continue;
        };
        let fields = fields.split_whitespace().collect::<Vec<_>>();
        let parent = fields.get(1).and_then(|value| value.parse().ok());
        let started = fields.get(19).and_then(|value| value.parse().ok());
        if let (Some(parent), Some(started)) = (parent, started) {
            table.insert(pid, (parent, started));
        }
    }
    (table, None)
}

/// Parent id and start time of every live process, and the id of the PowerShell lister to ignore.
#[cfg(windows)]
fn process_table() -> (BTreeMap<u32, Process>, Option<u32>) {
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$PID; Get-CimInstance Win32_Process | ForEach-Object { $started = if ($_.CreationDate) { $_.CreationDate.ToFileTimeUtc() } else { 0 }; '{0} {1} {2}' -f $_.ProcessId, $_.ParentProcessId, $started }",
        ])
        .output()
        .assert_value();
    assert!(output.status.success(), "listing Windows processes failed");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut lines = text.lines();
    let lister = lines.next().and_then(|line| line.trim().parse().ok());
    let table = lines
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            let started = fields.next()?.parse().ok()?;
            Some((pid, (parent, started)))
        })
        .collect();
    (table, lister)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn process_table() -> (BTreeMap<u32, Process>, Option<u32>) {
    panic!("the native process lifecycle is proven on Linux and Windows");
}

/// Every live descendant of the test process, without the lister and its own children. A process
/// counts as a child only when it started after its parent: Windows keeps the parent id of an
/// exited parent, and that id may meanwhile belong to another process.
fn descendants_in(table: &BTreeMap<u32, Process>, lister: Option<u32>) -> BTreeSet<Process> {
    let current = std::process::id();
    let mut found = BTreeSet::new();
    let mut frontier = vec![(current, table.get(&current).map_or(0, |entry| entry.1))];
    while let Some((parent, parent_started)) = frontier.pop() {
        for (&pid, &(owner, started)) in table {
            if owner == parent
                && pid != parent
                && started >= parent_started
                && Some(pid) != lister
                && found.insert((pid, started))
            {
                frontier.push((pid, started));
            }
        }
    }
    found
}

fn descendants() -> BTreeSet<Process> {
    let (table, lister) = process_table();
    descendants_in(&table, lister)
}

/// Processes other than the test that carry `marker` in their environment.
#[cfg(target_os = "linux")]
fn holders(marker: &str) -> Vec<u32> {
    let current = std::process::id();
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").assert_value().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == current {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path().join("environ")) else {
            continue;
        };
        if String::from_utf8_lossy(&bytes).contains(marker) {
            found.push(pid);
        }
    }
    found
}

#[cfg(not(target_os = "linux"))]
fn holders(_marker: &str) -> Vec<u32> {
    Vec::new()
}

/// Waits up to ten seconds until no process seen during the turn is alive and the test process
/// has no descendant left; on Linux no process may carry the node directory in its environment.
async fn assert_no_session_process(during: &BTreeSet<Process>, directory: &TestDirectory) {
    let marker = directory.path().to_string_lossy().into_owned();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (table, lister) = process_table();
        let alive = during
            .iter()
            .copied()
            .filter(|(pid, started)| table.get(pid).map(|entry| entry.1) == Some(*started))
            .collect::<Vec<_>>();
        let children = descendants_in(&table, lister);
        let holding = holders(&marker);
        if alive.is_empty() && children.is_empty() && holding.is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "OpenCode processes outlived the turn: alive {alive:?}, children {children:?}, holding {holding:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Waits for the provider's first request and returns the test's descendants at that moment.
async fn first_request<F>(
    requests: &mut mpsc::UnboundedReceiver<String>,
    execution: &mut F,
) -> BTreeSet<Process>
where
    F: std::future::Future<Output = Result<WorkerOutcome, NodeRunnerError>> + Unpin,
{
    tokio::select! {
        request = tokio::time::timeout(Duration::from_secs(120), requests.recv()) => {
            request.assert_value().assert_value();
        }
        result = execution => panic!("native turn ended before its provider request: {result:?}"),
    }
    let during = descendants();
    assert!(
        !during.is_empty(),
        "no OpenCode process was running during the turn"
    );
    during
}

#[tokio::test]
#[ignore = "requires installed OpenCode 2.4.0 or newer on Linux or Windows; localhost provider and a dummy key only"]
async fn native_a_completed_turn_leaves_no_opencode_process() {
    let directory = TestDirectory::new("opencode-lifecycle-completed");
    let (listener, endpoint) = listener().await;
    let (gate, open) = watch::channel(false);
    let (seen, mut requests) = mpsc::unbounded_channel();
    let server = tokio::spawn(provider(listener, Reply::Answer, seen, open));
    let adapter = declared_node(&directory);
    let (_cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let execution = adapter.run(declared(&endpoint), control);
    tokio::pin!(execution);
    let during = first_request(&mut requests, &mut execution).await;
    gate.send(true).assert_value();
    let result = tokio::time::timeout(Duration::from_secs(120), &mut execution)
        .await
        .assert_value();
    assert!(result.is_ok(), "{result:?}");
    drain.await.assert_value();
    server.abort();
    assert_no_session_process(&during, &directory).await;
}

#[tokio::test]
#[ignore = "requires installed OpenCode 2.4.0 or newer on Linux or Windows; localhost provider and a dummy key only"]
async fn native_a_cancelled_turn_leaves_no_opencode_process() {
    let directory = TestDirectory::new("opencode-lifecycle-cancelled");
    let (listener, endpoint) = listener().await;
    let (_gate, open) = watch::channel(false);
    let (seen, mut requests) = mpsc::unbounded_channel();
    let server = tokio::spawn(provider(listener, Reply::Hold, seen, open));
    let adapter = declared_node(&directory);
    let (cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let execution = adapter.run(declared(&endpoint), control);
    tokio::pin!(execution);
    let during = first_request(&mut requests, &mut execution).await;
    cancel.send(true).assert_value();
    let result = tokio::time::timeout(Duration::from_secs(30), &mut execution)
        .await
        .assert_value();
    assert!(
        matches!(result, Err(NodeRunnerError::Cancelled)),
        "{result:?}"
    );
    drain.await.assert_value();
    server.abort();
    assert_no_session_process(&during, &directory).await;
}

/// The supervisor enforces a node's `timeout_ms` by cancelling the node's handle
/// (`native_v2_supervisor::runtime::observe_dispatch`). Here the deadline passes while the
/// provider has started its answer and then stays silent.
#[tokio::test]
#[ignore = "requires installed OpenCode 2.4.0 or newer on Linux or Windows; localhost provider and a dummy key only"]
async fn native_a_turn_past_its_deadline_leaves_no_opencode_process() {
    let directory = TestDirectory::new("opencode-lifecycle-deadline");
    let (listener, endpoint) = listener().await;
    let (_gate, open) = watch::channel(false);
    let (seen, mut requests) = mpsc::unbounded_channel();
    let server = tokio::spawn(provider(listener, Reply::Stall, seen, open));
    let adapter = declared_node(&directory);
    let (cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let execution = adapter.run(declared(&endpoint), control);
    tokio::pin!(execution);
    let during = first_request(&mut requests, &mut execution).await;
    let early = tokio::time::timeout(Duration::from_secs(3), &mut execution).await;
    assert!(
        early.is_err(),
        "the stalled turn ended before its deadline: {early:?}"
    );
    cancel.send(true).assert_value();
    let result = tokio::time::timeout(Duration::from_secs(30), &mut execution)
        .await
        .assert_value();
    assert!(
        matches!(result, Err(NodeRunnerError::Cancelled)),
        "{result:?}"
    );
    drain.await.assert_value();
    server.abort();
    assert_no_session_process(&during, &directory).await;
}

/// One node with a saved login in its own user profile, and the provider it talks to.
struct Lane {
    key: String,
    user: TestDirectory,
    directory: TestDirectory,
    environment: BTreeMap<String, String>,
    adapter: OpenCodeAdapter,
    server: tokio::task::JoinHandle<()>,
}

/// The XDG data directory of the node's private store: the only `opencode.db` below the node.
fn private_store(node: &Path) -> PathBuf {
    let mut found = Vec::new();
    let mut pending = vec![node.to_owned()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.file_name().is_some_and(|name| name == "opencode.db") {
                found.push(path);
            }
        }
    }
    assert_eq!(
        found.len(),
        1,
        "expected one private store below {}: {found:?}",
        node.display()
    );
    found[0]
        .parent()
        .and_then(Path::parent)
        .assert_value()
        .to_owned()
}

async fn saved_turn(adapter: &OpenCodeAdapter) -> Result<WorkerOutcome, NodeRunnerError> {
    let (_cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let result = tokio::time::timeout(
        Duration::from_secs(120),
        adapter.run(invocation(&[]), control),
    )
    .await
    .assert_value();
    drain.await.assert_value();
    result
}

#[tokio::test]
#[ignore = "requires installed OpenCode 2.4.0 or newer on Linux or Windows; two local user profiles with dummy keys"]
async fn native_two_concurrent_nodes_keep_their_own_login_and_store() {
    let (gate, open) = watch::channel(false);
    let mut lanes = Vec::new();
    let mut receivers = Vec::new();
    for name in ["a", "b"] {
        let user = TestDirectory::new(&format!("opencode-lifecycle-user-{name}"));
        let (listener, endpoint) = listener().await;
        let (seen, requests) = mpsc::unbounded_channel();
        let server = tokio::spawn(provider(listener, Reply::Answer, seen, open.clone()));
        let config = json!({"provider":{"anthropic":{"options":{"baseURL":endpoint},"models":{"opaque/model":model()}}}});
        let environment = profile(&user, config);
        let key = format!("saved-key-{name}");
        let seed = json!([{"id":format!("cred_{name}"),"integrationID":"anthropic","label":name,"active":true,"value":{"type":"key","key":key}}]);
        let seed_file = user.child("seed.json");
        std::fs::write(&seed_file, seed.to_string()).assert_value();
        let status = opencode(user.path(), &environment)
            .args(["auth", "import", "--standalone"])
            .arg(&seed_file)
            .status()
            .assert_value();
        assert!(status.success(), "seeding user profile {name} failed");
        let directory = TestDirectory::new(&format!("opencode-lifecycle-node-{name}"));
        let adapter = node(&directory, user.path(), &environment);
        receivers.push(requests);
        lanes.push(Lane {
            key,
            user,
            directory,
            environment,
            adapter,
            server,
        });
    }

    let turns =
        async { tokio::join!(saved_turn(&lanes[0].adapter), saved_turn(&lanes[1].adapter)) };
    tokio::pin!(turns);
    // Both requests arrive while neither has been answered, so both nodes run at the same time.
    let mut heads = [Vec::new(), Vec::new()];
    for (index, receiver) in receivers.iter_mut().enumerate() {
        tokio::select! {
            head = tokio::time::timeout(Duration::from_secs(120), receiver.recv()) => {
                heads[index].push(head.assert_value().assert_value());
            }
            results = &mut turns => panic!("a node finished before both requests were in flight: {results:?}"),
        }
    }
    let during = descendants();
    assert!(
        !during.is_empty(),
        "no OpenCode process was running during the turns"
    );
    for (index, lane) in lanes.iter().enumerate() {
        let other = &lanes[1 - index].key;
        let mut environment = lane.environment.clone();
        environment.insert(
            "XDG_DATA_HOME".to_owned(),
            private_store(lane.directory.path())
                .to_string_lossy()
                .into_owned(),
        );
        let exported = opencode(lane.user.path(), &environment)
            .args(["auth", "export", "--standalone", "anthropic"])
            .output()
            .assert_value();
        assert!(
            exported.status.success(),
            "exporting the private store of node {index} failed"
        );
        let exported = String::from_utf8_lossy(&exported.stdout).into_owned();
        assert!(exported.contains(&lane.key), "node {index}: {exported}");
        assert!(
            !exported.contains(other.as_str()),
            "node {index}: {exported}"
        );
    }

    gate.send(true).assert_value();
    let (first, second) = (&mut turns).await;
    assert!(first.is_ok(), "{first:?}");
    assert!(second.is_ok(), "{second:?}");
    for (index, receiver) in receivers.iter_mut().enumerate() {
        while let Ok(head) = receiver.try_recv() {
            heads[index].push(head);
        }
    }
    for (index, lane) in lanes.iter().enumerate() {
        let other = &lanes[1 - index].key;
        for head in &heads[index] {
            assert!(
                head.contains(&format!("x-api-key: {}", lane.key)),
                "node {index}: {head}"
            );
            assert!(!head.contains(other.as_str()), "node {index}: {head}");
        }
    }
    for lane in &lanes {
        lane.server.abort();
        assert_no_session_process(&during, &lane.directory).await;
    }
}
