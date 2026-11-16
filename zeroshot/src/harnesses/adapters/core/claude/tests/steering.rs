use super::*;

/// A Claude Code that behaves like 2.1.310 in the respects the adapter relies on: the prompt is
/// one stream-json line and stdin stays open; a tool call runs the `PreToolUse` hook from
/// `--settings` settings; a user message written to stdin meanwhile enters the conversation after that
/// tool call and is replayed on stdout; a message still queued when a turn ends starts another
/// turn with a result of its own; the process exits only once stdin has closed.
const STEERED_CLAUDE: &str = r#"#!/usr/bin/env python3
import json, os, pathlib, select, shlex, socket, struct, sys, time
args = sys.argv[1:]
if args == ['--version']:
    print('2.1.318 (Claude Code)'); sys.exit(0)
if '--safe-mode' in args: sys.exit(1)
assert args[args.index('--input-format') + 1] == 'stream-json' and '--replay-user-messages' in args
def emit(record):
    print(json.dumps(record), flush=True)
def replay(line):
    record = json.loads(line); record['isReplay'] = True; emit(record)
def result():
    emit({'type':'result','subtype':'success','is_error':False,'result':'{"response":"done"}','session_id':'steer-session'})
mode = os.environ['STEER_MODE']
prompt = sys.stdin.readline()
pathlib.Path('prompt.json').write_text(prompt)
emit({'type':'system','subtype':'init','session_id':'steer-session'})
replay(prompt)
pathlib.Path('held').touch()
while not pathlib.Path('release').exists(): time.sleep(.01)
settings = json.loads(args[args.index('--settings') + 1])
hook = settings['hooks']['PreToolUse'][0]['hooks'][0]['command']
# The hook command is the product's own helper; this is its exchange with the run's broker.
capability = json.loads(pathlib.Path(shlex.split(hook)[-1]).read_text())
def call_hook(event):
    host, port = capability['endpoint'].rsplit(':', 1)
    with socket.create_connection((host, int(port)), timeout=5) as broker:
        body = json.dumps({'token': capability['token'], 'event': event}).encode()
        broker.sendall(struct.pack('>I', len(body)) + body)
        length = struct.unpack('>I', broker.recv(4, socket.MSG_WAITALL))[0]
        context = broker.recv(length, socket.MSG_WAITALL) if length else b''
        if context: broker.sendall(b'\x01')
        return context.decode()
contexts = []
for call in range(2):
    event = {'hook_event_name':'PreToolUse','tool_name':'Read','tool_input':{'file_path':'README.md'}}
    if call == 0: event['agent_id'] = 'a456838012fb65c82'
    contexts.append(call_hook(event))
pathlib.Path('hook-output.json').write_text(json.dumps(contexts))
queued = sys.stdin.readline() if select.select([sys.stdin], [], [], 5)[0] else ''
pathlib.Path('steer.json').write_text(queued)
if mode == 'after_result':
    result()
if queued and mode != 'never_shown':
    replay(queued)
result()
rest = sys.stdin.read()
pathlib.Path('closed').write_text(rest)
"#;

struct Steered {
    workspace: TestDirectory,
    receipts: usize,
    outcome: Result<WorkerOutcome, NodeRunnerError>,
}

/// One turn of the fake above with a maximum-size steer recorded while its first tool call waits.
async fn steered_turn(mode: &str) -> Steered {
    use crate::active_memory::hooks::ActiveMemoryHooks;
    use crate::harnesses::{HarnessBuildContext, HarnessHost, HarnessRegistry};
    use crate::native_v2_candidate::{NativeV2CandidateConfig, build_local_native_v2_candidate};
    use crate::v2_run_ledger::{CreateRun, RunLedger};
    use openengine_cluster_protocol::{RunId, RunSteerParams, RunVectorContent, Sha256Digest};
    let workspace = TestDirectory::new("claude-steer");
    let executable = workspace.write_executable(
        "claude",
        &openengine_cluster_testkit::fixture::python3_script(STEERED_CLAUDE),
    );
    let binding = agent_binding(
        "claude-opus-5",
        None,
        SessionScope::NodeInstance,
        &[ANTHROPIC_KEY, "STEER_MODE"],
    );
    let admitted = admitted_claude(ClaudeProvider::Anthropic, binding.clone(), false).await;
    let run = RunId::new("claude-run");
    let ledger: Arc<dyn RunLedger> = Arc::new(crate::v2_run_ledger::fake::FakeRunLedger::new());
    ledger
        .create_or_get(CreateRun {
            run_id: run.clone(),
            submission_key: IdempotencyKey::new("steer-claude").assert_value(),
            submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
            admitted: admitted.clone(),
        })
        .await
        .assert_value();
    let hooks = Arc::new(ActiveMemoryHooks::empty().assert_value());
    hooks.bind_ledger(run.clone(), ledger.clone());
    crate::execution::platform::private_directory(&workspace.child("runtime")).assert_value();
    let lane = HarnessRegistry::local()
        .configure(HarnessBuildContext {
            lane: admitted.runtime.lane().clone(),
            workspace: workspace.path().to_path_buf(),
            runtime_home: workspace.child("runtime"),
            executable,
            prefix_arguments: vec![],
            search_path: "/usr/bin:/bin".into(),
            base_environment: BTreeMap::new(),
            host: HarnessHost::Local {
                home: Some(workspace.child("home")),
                environment: LocalHarnessEnvironment::new(BTreeMap::new()),
            },
        })
        .assert_value();
    let runner = build_local_native_v2_candidate(
        &admitted,
        NativeV2CandidateConfig {
            active_memory: Some(hooks),
            lanes: vec![lane],
            workspace: workspace.path().to_path_buf(),
            delivery: None,
            comment_delivery: None,
        },
    )
    .assert_value();
    let handle = runner
        .start(request(
            binding,
            1,
            &[(ANTHROPIC_KEY, "anthropic-fake"), ("STEER_MODE", mode)],
        ))
        .await
        .assert_value();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !workspace.child("held").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .assert_value();
    crate::v2_run_ledger::steering::record(
        ledger.as_ref(),
        RunSteerParams {
            run_id: run.clone(),
            text: "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES),
        },
    )
    .await
    .assert_value();
    workspace.write("release", "");
    let (_, outcome) = super::failure::complete_with_durable(handle).await;
    let receipts = crate::v2_run_ledger::steering::all(ledger.as_ref(), &run)
        .await
        .assert_value()
        .iter()
        .filter(|vector| matches!(vector.content, RunVectorContent::SteerReceipt { .. }))
        .count();
    runner.close_run(&run).await;
    Steered {
        workspace,
        receipts,
        outcome: outcome.map(|completion| completion.outcome),
    }
}

fn assert_done(outcome: &Result<WorkerOutcome, NodeRunnerError>) {
    assert!(
        matches!(outcome, Ok(WorkerOutcome::Verified { output, .. }) if *output == json!("done")),
        "{outcome:?}"
    );
}

// The steer is no hook context: it is written to stdin whole as a user message, also when a
// subagent's tool call asked, and it is received once Claude Code has replayed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_reaches_claude_whole_on_stdin_and_is_received_when_replayed() {
    for mode in ["mid_turn", "after_result"] {
        let turn = steered_turn(mode).await;
        assert_done(&turn.outcome);
        let hook_output: Vec<String> =
            serde_json::from_str(&turn.workspace.read("hook-output.json")).assert_value();
        assert_eq!(hook_output, ["", ""], "the hook carries no steer");
        let steer = prompt_text(&turn.workspace.read("steer.json"));
        assert!(steer.starts_with("<steer>\n") && steer.ends_with("</steer>"));
        assert!(
            steer.contains(&"x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES))
        );
        assert!(!prompt_text(&turn.workspace.read("prompt.json")).contains("<steer>"));
        assert_eq!(turn.receipts, 1, "{mode}");
        // Stdin closed only after the turn that took the steer had ended, with nothing more sent.
        assert_eq!(turn.workspace.read("closed"), "");
    }
}

// A message Claude Code never takes is not received, and stdin still closes at the result.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_claude_never_replays_is_not_received_and_does_not_hold_the_turn() {
    let turn = steered_turn("never_shown").await;
    assert_done(&turn.outcome);
    assert_eq!(turn.receipts, 0);
    assert!(turn.workspace.child("closed").exists());
}
