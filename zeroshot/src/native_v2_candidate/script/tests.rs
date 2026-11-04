#![cfg(unix)]

use super::*;
use openengine_cluster_protocol::{EnvironmentVariableName, PayloadType};
use openengine_cluster_testkit::fixture::python3_script;
use serde_json::{Value, json};
use crate::native_v2_candidate::test_support::{NodeRequestFixture, TestDirectory};
use crate::native_v2_runner::NodeRole;

async fn invocation(
    adapter: &ScriptAdapter,
    command: Vec<String>,
    input: Value,
) -> DriverInvocation {
    let binding = serde_json::from_value(json!({
        "kind": "script", "command": command, "connections": {"fixture": ["SCRIPT_TOKEN"]}
    }))
    .unwrap();
    let request = NodeRequestFixture {
        run_id: "script-test",
        node: "script",
        node_instance: 1,
        execution: 1,
        worker: "test.script@1",
        instructions: "unused",
        input,
        binding,
        environment: BTreeMap::from([(
            EnvironmentVariableName::new("SCRIPT_TOKEN").unwrap(),
            "private-script-fixture-key".to_owned(),
        )]),
    }
    .into_request();
    let mut node = request.invocation;
    node.instructions = None;
    let session = adapter.open(&node, &request.environment).await.unwrap();
    DriverInvocation {
        dispatch: Default::default(),
        host_context: None,
        node,
        role: NodeRole::Worker,
        response: NodeResponseContract::Worker {
            output: serde_json::from_value::<PayloadType>(json!({
                "kind": "record", "fields": {"result": {"type": {"kind":"string"}, "required":true}}
            }))
            .unwrap(),
        },
        environment: request.environment,
        session,
        provider_session_slot: crate::native_v2_contract::NodeInstanceId::new(1).unwrap(),
    }
}

async fn prepared(directory: &TestDirectory, source: &str) -> (ScriptAdapter, DriverInvocation) {
    let executable = directory.write_executable(
        "worker",
        &python3_script(&format!(
            "#!/usr/bin/env python3\nimport sys\nsys.stdin.read()\n{source}"
        )),
    );
    let adapter = ScriptAdapter::new(directory.path().to_owned(), None);
    let request = invocation(
        &adapter,
        vec![executable.to_string_lossy().into_owned()],
        json!({}),
    )
    .await;
    (adapter, request)
}

#[tokio::test]
async fn script_exchanges_large_json_preserves_literal_arguments_and_redacts_stderr() {
    let directory = TestDirectory::new("script-json");
    let executable = directory.write_executable("worker", &python3_script(
        "#!/usr/bin/env python3\nimport json, os, sys\npayload = json.load(sys.stdin)\nassert os.environ['ZEROSHOT_NODE_NAME'] == 'script'\nassert os.environ['ZEROSHOT_NODE_KIND'] == 'step'\nassert os.environ['ZEROSHOT_NODE_ATTEMPT'] == '1'\nprint(os.environ['SCRIPT_TOKEN'], file=sys.stderr)\nprint(json.dumps({'result': payload['text'] + sys.argv[1]}))\n"
    ));
    let adapter = ScriptAdapter::new(directory.path().to_owned(), None);
    let literal = "$(touch must-not-exist); literal argument";
    let data = "x".repeat(1024 * 1024);
    let invocation = invocation(
        &adapter,
        vec![
            executable.to_string_lossy().into_owned(),
            literal.to_owned(),
        ],
        json!({"text":data}),
    )
    .await;
    let (_cancel, control, mut durable) =
        crate::native_v2_runner::test_support::control_with_capacity(16);
    let result = adapter.run(invocation, control).await.unwrap();
    let WorkerOutcome::Verified { output, .. } = result else {
        panic!("expected script success")
    };
    assert_eq!(output["result"], format!("{data}{literal}"));
    assert!(!directory.child("must-not-exist").exists());
    assert_eq!(
        durable
            .recv_usage()
            .await
            .unwrap()
            .unwrap()
            .input_tokens
            .get(),
        0
    );
    let log = durable.recv_output().await.unwrap();
    assert!(!log.text.contains("private-script-fixture-key"));
    assert!(log.text.contains("[redacted]"));
}

#[tokio::test]
async fn invalid_json_wrong_schema_nonzero_exit_and_oversized_output_fail() {
    for (label, source, expected) in [
        ("json", "print('not JSON')\n", WorkerErrorCode::Malformed),
        (
            "schema",
            "print('{\"result\":42}')\n",
            WorkerErrorCode::Malformed,
        ),
        (
            "exit",
            "import sys\nprint('{\"result\":\"ok\"}')\nsys.exit(7)\n",
            WorkerErrorCode::Crash,
        ),
        (
            "size",
            "print('x' * (8 * 1024 * 1024 + 1))\n",
            WorkerErrorCode::Malformed,
        ),
    ] {
        let directory = TestDirectory::new(label);
        let (adapter, invocation) = prepared(&directory, source).await;
        let (_cancel, control, _durable) =
            crate::native_v2_runner::test_support::control_with_capacity(16);
        let result = adapter.run(invocation, control).await.unwrap();
        assert!(
            matches!(result, WorkerOutcome::Error { code, .. } if code == expected),
            "{label}: {result:?}"
        );
        result.validate().unwrap();
    }
}

#[tokio::test]
async fn cancellation_settles_the_owned_script_process() {
    use std::time::Duration;
    let directory = TestDirectory::new("script-cancel");
    let (adapter, invocation) = prepared(
        &directory,
        "import pathlib, time\npathlib.Path('started').write_text('ready')\ntime.sleep(60)\n",
    )
    .await;
    let (cancel, control, _durable) =
        crate::native_v2_runner::test_support::control_with_capacity(16);
    let task = tokio::spawn(async move { adapter.run(invocation, control).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !directory.child("started").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    cancel.send(true).unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap(),
        Err(NodeRunnerError::Cancelled)
    ));
}

#[tokio::test]
async fn truncated_stderr_cannot_publish_a_credential_fragment() {
    let directory = TestDirectory::new("script-truncated-secret");
    let (adapter, invocation) = prepared(&directory,
        "import os\nprint(os.environ['SCRIPT_TOKEN'] + 'x' * (64 * 1024 - 8), file=sys.stderr)\nprint('{\"result\":\"ok\"}')\n").await;
    let (_cancel, control, mut durable) =
        crate::native_v2_runner::test_support::control_with_capacity(16);
    let outcome = adapter.run(invocation, control).await.unwrap();
    assert!(matches!(outcome, WorkerOutcome::Verified { .. }));
    let diagnostic = durable.recv_output().await.unwrap();
    assert!(diagnostic.text.contains("diagnostics omitted"));
    assert!(!diagnostic.text.contains("ture-key"));
    assert!(!diagnostic.text.contains("xxxxxxxx"));
}
