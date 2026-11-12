use std::collections::BTreeMap;
use std::sync::Arc;

use openengine_cluster_protocol::WorkerOutcome;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_runner::test_support::control;

mod command_tests;

fn adapter(directory: &TestDirectory, fixture: &str) -> HermesAdapter {
    let source = openengine_cluster_testkit::fixture::python3_script(fixture);
    let executable = directory.write_executable("hermes", &source);
    let workspace = directory.child("workspace");
    std::fs::create_dir_all(&workspace).assert_value();
    std::fs::create_dir_all(directory.child("runtime")).assert_value();
    HermesAdapter::new_local(HermesConfig {
        provider: "openai".to_owned(),
        executable: executable.to_string_lossy().into_owned(),
        prefix_arguments: vec![],
        workspace,
        runtime_home: directory.child("runtime"),
        local_user_home: Some(directory.path().to_owned()),
        native_environment: LocalHarnessEnvironment::default(),
        base_environment: crate::native_v2_capsule::HarnessProcessEnvironment::new(BTreeMap::from(
            [("PATH".to_owned(), "/usr/bin:/bin".to_owned())],
        ))
        .assert_value(),
        process_pool: HostedProcessPool::hosted_default(),
    })
}

fn invocation(fields: &[(&str, &str)]) -> DriverInvocation {
    crate::harnesses::adapters::contributed::test_support::invocation(
        "hermes",
        fields,
        Arc::new(HermesSession {
            core: ProviderSessionCore::new(),
            id: "owned-session".to_owned(),
        }),
    )
}

#[cfg(unix)]
fn fixture(answer: &str, clean: bool) -> String {
    format!(
        r##"#!/usr/bin/env python3
import json, os, pathlib, sys
assert sys.argv[1:] == ['--version', '--help']
def emit(value): print(json.dumps(value), flush=True)
emit({{'type':'ready','protocol':1}})
init=json.loads(sys.stdin.readline())
assert init['model']=='opaque/model'
assert not os.environ.get('OPENAI_API_KEY')
assert init['providerEnvironment'].get('OPENAI_API_KEY') == init['authority'].get('apiKey')
assert 'OPENAI_API_KEY' not in init['declaredEnvironment']
assert 'OPENAI_API_KEY' not in init['nativeEnvironment']
if 'ANTHROPIC_API_KEY' in init['nativeEnvironment']:
    assert not os.environ.get('ANTHROPIC_API_KEY')
assert not any(name.upper().startswith('ZEROSHOT_') for name in init['nativeEnvironment'])
assert not init['contained']
assert os.environ['HOME'] != str(pathlib.Path(init['dbPath']).parent)
state=pathlib.Path(init['dbPath']+'.fixture')
pathlib.Path('native-home').write_text(str(pathlib.Path(init['dbPath']).parent))
previous=json.loads(state.read_text()) if state.exists() else []
assert not previous or previous[-1]==init['sessionId']
previous.append(init['sessionId']); state.write_text(json.dumps(previous))
emit({{'type':'initialized','sessionId':init['sessionId']}})
for line in sys.stdin:
    message=json.loads(line)
    if message['type']=='stop':
        if {clean}: emit({{'type':'closed','clean':True}})
        break
    assert message['type']=='prompt'
    emit({{'type':'text','text':init['authority'].get('apiKey') or 'native text'}})
    emit({{'type':'result','message':{answer},'completed':True,'failed':False,'interrupted':False,
           'usage':{{'input':11,'output':7}}}})
"##,
        answer = json!(answer),
        clean = if clean { "True" } else { "False" }
    )
}

async fn run(
    adapter: &HermesAdapter,
    invocation: DriverInvocation,
) -> Result<WorkerOutcome, NodeRunnerError> {
    let (_cancel, control, mut durable) = control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let result = adapter.run(invocation, control).await;
    drain.await.assert_value();
    result
}

#[cfg(unix)]
#[tokio::test]
async fn native_uncached_input_is_preserved_with_both_cache_buckets() {
    let directory = TestDirectory::new("hermes-uncached-usage");
    let source = fixture("{\"response\":null}", true).replace(
        "'input':11,'output':7",
        "'input':2,'output':3,'cacheRead':5,'cacheWrite':7",
    );
    let adapter = adapter(&directory, &source);
    let (_cancel, control, mut durable) = control();
    let drain = async {
        let usage = durable.recv_usage().await.assert_value().assert_value();
        while durable.recv().await.is_ok() {}
        usage
    };
    let (outcome, usage) = tokio::join!(adapter.run(invocation(&[]), control), drain);
    assert!(matches!(
        outcome.assert_value(),
        WorkerOutcome::Verified { .. }
    ));
    assert_eq!(usage.input_tokens.get(), 2);
    assert_eq!(usage.output_tokens.get(), 3);
    assert_eq!(usage.cache_read_input_tokens.assert_value().get(), 5);
    assert_eq!(usage.cache_creation_input_tokens.assert_value().get(), 7);
}

#[cfg(unix)]
#[tokio::test]
async fn private_handshake_credentials_and_native_node_session_reuse() {
    let directory = TestDirectory::new("hermes-session");
    let mut source = fixture("{\"response\":null}", true).replace(
        "assert message['type']=='prompt'",
        "assert message['type']=='prompt'; pathlib.Path('received-prompt.txt').write_text(message['text'])",
    );
    source.push_str(
        "\nassert init['nativeEnvironment']['ANTHROPIC_API_KEY']=='unrelated-native-login'\n",
    );
    let mut adapter = adapter(&directory, &source);
    adapter.config.native_environment = LocalHarnessEnvironment::new(BTreeMap::from([
        (
            "OPENAI_API_KEY".to_owned(),
            "ambient-selected-login".to_owned(),
        ),
        (
            "ANTHROPIC_API_KEY".to_owned(),
            "unrelated-native-login".to_owned(),
        ),
        ("ZEROSHOT_ORGANIZATION".to_owned(), "host-only".to_owned()),
    ]));
    adapter.local_environment = adapter
        .config
        .native_environment
        .selected(command::LOCAL_ENVIRONMENT);
    let mut invocation = invocation(&[("OPENAI_API_KEY", "declared-sensitive-fixture")]);
    for context in ["first private context", "fresh private context"] {
        invocation.host_context = Some(context.to_owned());
        assert!(run(&adapter, invocation.clone()).await.is_ok());
        let prompt = std::fs::read_to_string(adapter.config.workspace.join("received-prompt.txt"))
            .assert_value();
        assert!(prompt.starts_with(&format!("Runtime-owned execution context:\n{context}\n\n")));
        assert_eq!(
            invocation.agent_instructions().assert_value().as_str(),
            "Exercise the adapter."
        );
    }
    drop(invocation);
    let home = std::fs::read_to_string(adapter.config.workspace.join("native-home")).assert_value();
    assert!(!PathBuf::from(home).join("native_runtime.py").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn native_cleanup_acknowledgement_is_required_even_after_success() {
    let directory = TestDirectory::new("hermes-unconfirmed-cleanup");
    let adapter = adapter(&directory, &fixture("{\"response\":null}", false));
    assert_eq!(
        run(&adapter, invocation(&[])).await,
        Err(NodeRunnerError::CleanupUnconfirmed)
    );
    let home = std::fs::read_to_string(adapter.config.workspace.join("native-home")).assert_value();
    assert!(
        PathBuf::from(home).join("native_runtime.py").is_file(),
        "unconfirmed native jobs must retain their files"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn structured_corrections_use_the_same_runtime_and_remain_bounded() {
    let directory = TestDirectory::new("hermes-corrections");
    let adapter = adapter(&directory, &fixture("malformed", true));
    assert_eq!(
        run(&adapter, invocation(&[])).await.assert_value(),
        WorkerOutcome::malformed()
    );
}

#[test]
fn connection_controls_cannot_replace_the_native_runtime() {
    for name in [
        "PATH",
        "PYTHONPATH",
        "pythonhome",
        "HERMES_HOME",
        "HERMES_SAFE_MODE",
        "ZEROSHOT_HERMES_BRIDGE",
    ] {
        assert!(command::validate_connection_name(name).is_err());
    }
    for name in ["OPENAI_API_KEY", "HERMES_EXEC_ASK", "MCP_NATIVE_TOKEN"] {
        command::validate_connection_name(name).assert_value();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn foreign_declared_credential_refuses_before_native_startup() {
    let directory = TestDirectory::new("hermes-foreign-key");
    let adapter = adapter(&directory, "#!/bin/sh\nexit 99\n");
    let result = run(
        &adapter,
        invocation(&[("ANTHROPIC_API_KEY", "foreign-credential")]),
    )
    .await;
    assert!(
        matches!(result, Err(NodeRunnerError::DriverDetail(detail)) if detail.contains("conflicts"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ignored_bootstrap_exits_without_receiving_a_prompt() {
    let directory = TestDirectory::new("hermes-ignored-bootstrap");
    let adapter = adapter(&directory, "#!/bin/sh\nprintf '%s\\n' 'Hermes help'\n");
    let result = run(&adapter, invocation(&[])).await;
    assert!(
        matches!(result, Err(NodeRunnerError::DriverDetail(detail)) if detail.contains("bootstrap"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn native_failure_keeps_the_explanation_and_redacts_declared_credentials() {
    let directory = TestDirectory::new("hermes-native-failure");
    let source = fixture("unused", true).replace(
        "'completed':True,'failed':False",
        "'completed':False,'failed':True,'error':init['authority']['apiKey']+' native authentication refused'",
    );
    let adapter = adapter(&directory, &source);
    let result = run(
        &adapter,
        invocation(&[("OPENAI_API_KEY", "declared-sensitive-fixture")]),
    )
    .await;
    assert!(matches!(result, Err(NodeRunnerError::DriverDetail(detail))
        if detail.contains("native authentication refused") && !detail.contains("declared-sensitive-fixture")));
}

#[cfg(unix)]
#[tokio::test]
async fn native_cleanup_failure_keeps_redacted_stage_evidence() {
    let directory = TestDirectory::new("hermes-cleanup-evidence");
    let source = fixture("{\"response\":null}", false).replace(
        "if False: emit({'type':'closed','clean':True})",
        "emit({'type':'error','message':init['authority']['apiKey']+' pending LSP'})",
    );
    let adapter = adapter(&directory, &source);
    let (_cancel, control, mut durable) = control();
    let drain = tokio::spawn(async move {
        let mut outputs = Vec::new();
        while let Ok(output) = durable.recv_output().await {
            outputs.push(output);
        }
        outputs
    });
    let outcome = adapter
        .run(
            invocation(&[("OPENAI_API_KEY", "declared-sensitive-fixture")]),
            control,
        )
        .await;
    assert_eq!(outcome, Err(NodeRunnerError::CleanupUnconfirmed));
    let outputs = drain.await.assert_value();
    assert!(
        outputs
            .iter()
            .any(|output| output.diagnostic.is_some() && output.text.contains("pending LSP"))
    );
    assert!(
        outputs
            .iter()
            .all(|output| !output.text.contains("declared-sensitive-fixture"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_waits_for_native_cleanup_of_a_detached_tool() {
    let directory = TestDirectory::new("hermes-cancel-detached-tool");
    let adapter = adapter(&directory, include_str!("tests/cancellable.py"));
    let marker = adapter.config.workspace.join("owned-child.pid");
    let (cancel, control, mut durable) = control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let running = tokio::spawn(async move { adapter.run(invocation(&[]), control).await });
    let pid = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(value) = std::fs::read_to_string(&marker) {
                break value.parse::<i32>().assert_value();
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .assert_value();
    cancel.send_replace(true);
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), running)
        .await
        .assert_value()
        .assert_value();
    assert_eq!(outcome, Err(NodeRunnerError::Cancelled));
    drain.await.assert_value();
    // SAFETY: kill with signal zero only inspects the captured fixture child's existence.
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "owned detached tool survived cancellation"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[tokio::test]
async fn native_options_are_isolated_and_preserved_on_revisits() {
    let directory = TestDirectory::new("hermes-native-config");
    let source=fixture("{\"response\":null}",true).replace("assert init['model']=='opaque/model'","assert init['model']=='opaque/model'\nwith open('native-options','a') as capture: capture.write(json.dumps(init['options'])+'\\n')");
    let adapter = adapter(&directory, &source);
    let mut configured = invocation(&[]);
    crate::harnesses::native_config::tests::set(
        &mut configured.node.binding,
        json!({"options":{"agent":{"max_turns":12},"futureNativeKey":true}}),
    );
    run(&adapter, configured.clone()).await.assert_value();
    run(&adapter, configured).await.assert_value();
    run(&adapter, invocation(&[])).await.assert_value();
    let configs = std::fs::read_to_string(adapter.config.workspace.join("native-options"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(configs.len(), 3);
    assert_eq!(configs[0]["agent"]["max_turns"], 12);
    assert_eq!(configs[1]["agent"]["max_turns"], 12);
    assert_eq!(configs[2], json!({}));
}

#[tokio::test]
async fn native_initialization_error_retains_cause_and_never_receives_prompt() {
    let directory = TestDirectory::new("hermes-native-rejection");
    let source = r#"#!/usr/bin/env python3
import json,sys
print(json.dumps({'type':'ready','protocol':1}),flush=True)
init=json.loads(sys.stdin.readline())
assert init['options']['agent']['max_turns']=='native-value'
print(json.dumps({'type':'error','message':'config.agent.max_turns: invalid native value'}),flush=True)
frame=json.loads(sys.stdin.readline())
assert frame['type']=='stop'
print(json.dumps({'type':'closed','clean':True}),flush=True)
"#;
    let adapter = adapter(&directory, source);
    let mut invocation = invocation(&[]);
    crate::harnesses::native_config::tests::set(
        &mut invocation.node.binding,
        json!({"options":{"agent":{"max_turns":"native-value"}}}),
    );
    let detail = run(&adapter, invocation).await.unwrap_err().to_string();
    assert!(
        detail.contains("hermes")
            && detail.contains("node `")
            && detail.contains("config.agent.max_turns")
            && detail.contains("invalid native value"),
        "{detail}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn dispatch_metadata_reaches_hermes_without_stale_native_overlays() {
    use crate::native_v2_candidate::test_support::assert_execution_environment;
    use crate::native_v2_runner::NodeDispatchMetadata;
    let directory = TestDirectory::new("hermes-dispatch-metadata");
    let source = fixture("{\"response\":null}", true).replace("assert init['model']=='opaque/model'", r#"
assert init['model']=='opaque/model'
names = ('ZEROSHOT_RUN_ID','ZEROSHOT_NODE_NAME','ZEROSHOT_NODE_KIND',
         'ZEROSHOT_NODE_INSTANCE_ID','ZEROSHOT_EXECUTION_REF','ZEROSHOT_NODE_ATTEMPT','ZEROSHOT_NODE_ITERATION')
pathlib.Path('execution-context').write_text(''.join(os.environ[name]+'\n' for name in names))
assert not any(name in init['declaredEnvironment'] for name in names)
"#);
    let adapter = adapter(&directory, &source);
    let mut visit = invocation(&[("ZEROSHOT_NODE_NAME", "spoofed")]);
    visit.dispatch = NodeDispatchMetadata {
        attempt: 2,
        iteration: 3,
    };
    run(&adapter, visit.clone()).await.assert_value();
    let capture =
        std::fs::read_to_string(adapter.config.workspace.join("execution-context")).assert_value();
    assert_execution_environment(&capture, &visit.node.reference, visit.dispatch, "step");
}
