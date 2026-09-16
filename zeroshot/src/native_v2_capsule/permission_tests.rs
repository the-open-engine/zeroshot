//! Exercises actual hosted adapters, including their configuration probes and private copies.
#![cfg(unix)]

use std::{collections::BTreeMap, fs, sync::Arc};
use openengine_cluster_protocol::{IdempotencyKey, NodeName, RunId, RunSize, RunTitle, WorkerOutcome};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};
use crate::execution::{SessionScope, process::HostedProcessPool};
use crate::native_v2_candidate::test_support::{
    NodeRequestFixture, TestDirectory, admit, environment_name, full_graph, success_node,
};
use crate::native_v2_claude::{ClaudeAdapter, ClaudeAdapterConfig, ClaudeProcessEnvironment};
use crate::native_v2_codex::{NativeV2CodexAdapter, NativeV2CodexConfig};
use crate::native_v2_contract::{
    AdmittedRun, ClaudeProvider, CodexProvider, DeclaredConnections, DeclaredEnvironment,
    NodeRuntimeBinding, RunSubmission, RuntimePlan,
};
use crate::native_v2_runner::{NativeNodeRunner, NodeRunner};
use crate::worker_catalog::ModelId;

#[tokio::test]
async fn root_hosted_harness_permission_defaults_preserve_policy_and_verifier_copies() {
    // SAFETY: geteuid only observes process identity.
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    for harness in ["codex", "claude"] {
        for verifier in [false, true] {
            for policy in ["unset", "configured", "unsupported"] {
                hosted_case(harness, verifier, policy, None).await;
            }
        }
    }
}

// Real configuration control traffic only: the fixture intercepts every model invocation.
// Run explicitly as root with the pinned CLI executable paths in these two variables.
#[tokio::test]
#[ignore = "requires root and ZEROSHOT_TEST_CODEX_EXECUTABLE/ZEROSHOT_TEST_CLAUDE_EXECUTABLE"]
async fn root_hosted_native_configuration_preserves_policy_through_correction() {
    // SAFETY: geteuid only observes process identity.
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "requires hosted root boundary"
    );
    for (harness, variable) in [
        ("codex", "ZEROSHOT_TEST_CODEX_EXECUTABLE"),
        ("claude", "ZEROSHOT_TEST_CLAUDE_EXECUTABLE"),
    ] {
        let executable = std::env::var(variable).assert_value_with(variable);
        assert!(std::path::Path::new(&executable).is_absolute());
        let special_policy = if harness == "codex" {
            "cached"
        } else {
            "sandbox"
        };
        for verifier in [false, true] {
            for policy in ["unset", "configured", "malformed", special_policy] {
                hosted_case(harness, verifier, policy, Some(&executable)).await;
            }
        }
    }
}

async fn hosted_case(harness: &str, verifier: bool, policy: &str, native: Option<&str>) {
    let fixture = TestDirectory::new("hosted-permission-default");
    let workspace = fixture.child("workspace");
    let runtime = fixture.child("runtime");
    fs::create_dir(&workspace).assert_value();
    fs::create_dir(&runtime).assert_value();
    let pool = HostedProcessPool::new(83_002, 83_002, 84_000, 84_000).assert_value();
    super::prepare_capsule_filesystem(super::CapsuleFilesystemSpec {
        workspace: &workspace,
        runtime_home: &runtime,
        process_pool: pool,
    })
    .assert_value();
    fixture.write_executable("harness", SCRIPT);
    let mut values = BTreeMap::from_iter(
        [
            ("HARNESS", harness.to_owned()),
            ("POLICY", policy.to_owned()),
            ("VERIFIER", verifier.to_string()),
            ("CANDIDATE", workspace.display().to_string()),
            (
                if harness == "codex" {
                    "OPENAI_API_KEY"
                } else {
                    "ANTHROPIC_API_KEY"
                },
                "fake-key".to_owned(),
            ),
        ]
        .map(|(name, value)| (environment_name(name), value)),
    );
    if let Some(executable) = native {
        values.insert(
            environment_name("NATIVE_CONFIGURATION_CLI"),
            executable.to_owned(),
        );
        for name in [
            "DISABLE_TELEMETRY",
            "DISABLE_ERROR_REPORTING",
            "DISABLE_AUTOUPDATER",
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
        ] {
            values.insert(environment_name(name), "1".to_owned());
        }
    }
    let binding = NodeRuntimeBinding::Agent {
        model: ModelId::new("opaque-model").assert_value(),
        effort: None,
        session_scope: SessionScope::Execution,
        connections: DeclaredConnections::single(
            "provider",
            DeclaredEnvironment::new(values.keys().cloned()).assert_value(),
        )
        .assert_value(),
    };
    let admitted = admission(harness, verifier, binding.clone()).await;
    let runner = hosted_runner(&fixture, harness, &admitted, pool);
    let mut handle = runner
        .start(
            NodeRequestFixture {
                run_id: "permission-test",
                node: "work",
                node_instance: 1,
                execution: 1,
                worker: "agent.work@1",
                instructions: "Check policy.",
                input: Value::Null,
                binding,
                environment: values,
            }
            .into_request(),
        )
        .await
        .assert_value();
    let mut output = handle.take_initial_output().assert_value();
    let (logs, completion) = tokio::join!(
        async {
            let mut logs = String::new();
            while let Ok(entry) = output.recv_output().await {
                logs.push_str(&entry.text);
            }
            logs
        },
        handle.completion()
    );
    assert!(
        matches!(
            completion.assert_value().outcome,
            WorkerOutcome::Verified { .. } | WorkerOutcome::Verifier { .. }
        ),
        "{harness} verifier={verifier} policy={policy}: {logs}"
    );
    assert!(!logs.contains("private-config-sentinel"));
    assert_eq!(workspace.join("inspection-proof").exists(), !verifier);
    assert_eq!(workspace.join("model-proof").exists(), !verifier);
    runner.close_run(&RunId::new("permission-test")).await;
}

fn hosted_runner(
    fixture: &TestDirectory,
    harness: &str,
    admitted: &AdmittedRun,
    pool: HostedProcessPool,
) -> NativeNodeRunner {
    match harness {
        "codex" => {
            let adapter = Arc::new(NativeV2CodexAdapter::new(NativeV2CodexConfig {
                provider: CodexProvider::OpenAi,
                executable: fixture.child("harness"),
                workspace: fixture.child("workspace"),
                runtime_home: fixture.child("runtime"),
                local_user: None,
                search_path: "/usr/bin:/bin".to_owned(),
                process_pool: pool,
            }));
            NativeNodeRunner::new(admitted, adapter.clone(), adapter).assert_value()
        }
        _ => {
            let adapter = Arc::new(
                ClaudeAdapter::new(ClaudeAdapterConfig {
                    provider: ClaudeProvider::Anthropic,
                    executable: fixture.child("harness").display().to_string(),
                    prefix_arguments: vec![],
                    workspace: fixture.child("workspace"),
                    runtime_home: fixture.child("runtime"),
                    local_user_home: None,
                    base_environment: ClaudeProcessEnvironment::new(BTreeMap::from([(
                        "PATH".to_owned(),
                        "/usr/bin:/bin".to_owned(),
                    )]))
                    .assert_value(),
                    process_pool: pool,
                })
                .assert_value(),
            );
            NativeNodeRunner::new(admitted, adapter.clone(), adapter).assert_value()
        }
    }
}

async fn admission(harness: &str, verifier: bool, binding: NodeRuntimeBinding) -> AdmittedRun {
    let mut node = json!({"kind":"step","name":"work","worker":"agent.work@1",
        "instructions":"Check policy.","input":{"kind":"null"},"output":{"kind":"null"},
        "inputBindings":[],"writeBindings":[],"attempts":1});
    if verifier {
        node["kind"] = json!("verifier");
        node["signals"] = json!({"verdict":["accepted","rejected"]});
        node["diagnostic"] = json!({"kind":"null"});
    }
    let nodes = BTreeMap::from([(NodeName::new("work").assert_value(), binding)]);
    let runtime = if harness == "codex" {
        RuntimePlan::Codex {
            provider: CodexProvider::OpenAi,
            size: RunSize::Medium,
            nodes,
        }
    } else {
        RuntimePlan::Claude {
            provider: ClaudeProvider::Anthropic,
            size: RunSize::Medium,
            nodes,
        }
    };
    admit(RunSubmission {
        title: RunTitle::new("Hosted permissions").assert_value(),
        graph: full_graph(vec![node, success_node()]),
        initial_input: Value::Null,
        runtime,
        source: serde_json::from_value(json!({"repository":"open-engine/zeroshot","branch":"main",
            "revision":"0123456789abcdef0123456789abcdef01234567"}))
        .assert_value(),
        submission_key: IdempotencyKey::new("hosted-permissions").assert_value(),
    })
    .await
}

const SCRIPT: &str = r#"#!/usr/bin/python3
import json, os, pathlib, subprocess, sys
# These are supplied only to the test host, never declared by its node connection.
for name in ['CODEX_BASE_URL','OPENAI_BASE_URL','ANTHROPIC_BASE_URL',
             'CLAUDE_CONFIG_DIR','CLAUDE_CODE_FORCE_SANDBOX']:
    assert name not in os.environ, 'host setting leaked into embedded provider: ' + name
harness, policy = os.environ['HARNESS'], os.environ['POLICY']
verifier = os.environ['VERIFIER'] == 'true'
probe = sys.argv[1:2] == ['app-server'] or '--safe-mode' in sys.argv
if probe:
    pathlib.Path('inspection-proof').write_text('inspected')
    if verifier:
        assert str(pathlib.Path.cwd()) != os.environ['CANDIDATE']
        try:
            pathlib.Path(os.environ['CANDIDATE'], 'forbidden').touch()
            raise AssertionError('candidate writable')
        except PermissionError:
            pass
    native = os.environ.get('NATIVE_CONFIGURATION_CLI')
    if native:
        assert '--model' not in sys.argv, 'model arguments reached native probe'
        home = pathlib.Path(os.environ['HOME'])
        config = (pathlib.Path(os.environ['CODEX_HOME'], 'config.toml')
            if harness == 'codex' else home / '.claude/settings.json')
        config.parent.mkdir(parents=True, exist_ok=True)
        if harness == 'codex':
            content = {'unset':'', 'configured':'approval_policy="never"\n',
                'cached':'web_search="cached"\n', 'malformed':'sandbox_mode=[\n'}[policy]
        else:
            settings = {'apiKeyHelper':'touch helper-ran', 'awsAuthRefresh':'touch helper-ran',
                'awsCredentialExport':'touch helper-ran', 'gcpAuthRefresh':'touch helper-ran',
                'proxyAuthHelper':'touch helper-ran', 'hooks':{'SessionStart':[{'hooks':[{
                    'type':'command','command':'touch hook-ran'}]}]}}
            if policy == 'configured': settings['permissions'] = {'defaultMode':'plan'}
            if policy == 'sandbox': settings['sandbox'] = {'enabled':False}
            content = '{"permissions":' if policy == 'malformed' else json.dumps(settings)
        if config.exists():
            assert config.read_text() == content, 'native query changed authored settings'
        else:
            config.write_text(content)
        pathlib.Path('native-config-proof').write_text(json.dumps({
            'path':str(config), 'content':content, 'home':str(home), 'uid':os.getuid()}))
        # The real native CLI receives only validated configuration requests. Model calls are
        # handled below by this fixture, even when a correction resumes the provider session.
        child = subprocess.Popen([native, *sys.argv[1:]], stdin=subprocess.PIPE, text=True)
        for line in sys.stdin:
            request = json.loads(line)
            if harness == 'codex':
                methods = ['initialize','initialized','config/read','configRequirements/read']
                assert request.get('method') in methods, request
            else:
                assert request.get('type') == 'control_request', request
                assert request['request']['subtype'] in ['initialize','get_settings'], request
            try:
                child.stdin.write(line)
                child.stdin.flush()
            except BrokenPipeError:
                break
        child.stdin.close()
        sys.exit(child.wait())
    if policy == 'unsupported':
        sys.exit(1)
    for line in sys.stdin:
        request = json.loads(line)
        if harness == 'codex':
            if 'id' not in request: continue
            result = {}
            if request['method'] == 'config/read':
                config = {'sandbox_mode':None,'approval_policy':None,'projects':None}
                if policy == 'configured': config['approval_policy'] = 'on-request'
                result = {'config':config,'origins':{},'layers':[]}
            elif request['method'] == 'configRequirements/read':
                result = {'requirements':None}
            response = {'id':request['id'],'result':result}
        else:
            result = {'current_permission_mode':'default'}
            if request['request']['subtype'] == 'get_settings':
                effective = {'env':{'EXAMPLE_SECRET':'private-config-sentinel'}}
                if policy == 'configured': effective['permissions'] = {'defaultMode':'plan'}
                result = {'effective':effective,'sources':[],'errors':[]}
            response = {'type':'control_response','response':{
                'subtype':'success','request_id':request['request_id'],'response':result}}
        print(json.dumps(response), flush=True)
    sys.exit(0)
assert pathlib.Path('inspection-proof').read_text() == 'inspected'
flag = '--dangerously-bypass-approvals-and-sandbox' if harness == 'codex' else '--dangerously-skip-permissions'
assert (flag in sys.argv) == (policy == 'unset'), sys.argv
assert '--sandbox' not in sys.argv and '--permission-mode' not in sys.argv, sys.argv
native = os.environ.get('NATIVE_CONFIGURATION_CLI')
count = 0
if native:
    proof = json.loads(pathlib.Path('native-config-proof').read_text())
    assert pathlib.Path(proof['path']).read_text() == proof['content'], 'settings changed before turn'
    assert proof['home'] == os.environ['HOME'] and proof['uid'] == os.getuid(), 'probe identity changed'
    assert not pathlib.Path('helper-ran').exists() and not pathlib.Path('hook-ran').exists()
    counter = pathlib.Path('model-count')
    count = int(counter.read_text()) if counter.exists() else 0
    counter.write_text(str(count + 1))
    assert count < 2, 'unexpected extra model correction'
    resumed = 'resume' in sys.argv if harness == 'codex' else '--resume' in sys.argv
    assert resumed == (count == 1), sys.argv
pathlib.Path('model-proof').touch()
sys.stdin.read()
response = {'output':None,'signals':{'verdict':'accepted'},'diagnostic':None} if verifier else None
if native and count == 0:
    response = 'invalid output to require correction'
if harness == 'codex':
    print(json.dumps({'type':'thread.started','thread_id':'test-thread'}))
    print(json.dumps({'type':'item.completed','item':{
        'type':'agent_message','text':json.dumps({'response':response})}}))
    print(json.dumps({'type':'turn.completed'}))
else:
    print(json.dumps({'type':'system','subtype':'init','session_id':'test-session'}))
    print(json.dumps({'type':'result','subtype':'success','is_error':False,'structured_output':{'response':response}}))
"#;
