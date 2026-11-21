#![cfg(unix)]

use super::*;
use openengine_cluster_protocol::{EnvironmentVariableName, PayloadType};
use openengine_cluster_testkit::fixture::python3_script;
use serde_json::{Value, json};
use crate::native_v2_candidate::test_support::{NodeRequestFixture, TestDirectory};
use crate::native_v2_runner::{NodeRole, NodeResponseContract};

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
        provider_home_slot: crate::native_v2_contract::NodeInstanceId::new(1).unwrap(),
        auxiliary: false,
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

async fn run_script(
    adapter: &ScriptAdapter,
    invocation: DriverInvocation,
) -> (WorkerOutcome, crate::native_v2_runner::DurableOutput) {
    let (_cancel, control, durable) =
        crate::native_v2_runner::test_support::control_with_capacity(16);
    (adapter.run(invocation, control).await.unwrap(), durable)
}

fn verifier(mut invocation: DriverInvocation) -> DriverInvocation {
    let NodeResponseContract::Worker { output } = invocation.response else {
        panic!("expected worker fixture")
    };
    invocation.role = NodeRole::Verifier;
    invocation.response = NodeResponseContract::Verifier {
        output,
        signals: serde_json::from_value(json!({"status":["ready","logged_out"]})).unwrap(),
        diagnostic: PayloadType::Null,
    };
    invocation
}

#[tokio::test]
async fn script_verifier_returns_declared_signals_and_successful_timing() {
    let directory = TestDirectory::new("script-verifier");
    let (adapter, invocation) = prepared(&directory,
        "import os\nassert os.environ['ZEROSHOT_NODE_KIND'] == 'verifier'\nprint('{\"output\":{\"result\":\"ok\"},\"signals\":{\"status\":\"logged_out\"},\"diagnostic\":null}')\n").await;
    let (outcome, mut durable) = run_script(&adapter, verifier(invocation)).await;
    let WorkerOutcome::Verifier {
        output,
        signals,
        diagnostic,
        ..
    } = outcome
    else {
        panic!("expected verifier success: {outcome:?}")
    };
    assert_eq!(output, json!({"result":"ok"}));
    assert_eq!(
        serde_json::to_value(signals).unwrap(),
        json!({"status":"logged_out"})
    );
    assert_eq!(diagnostic, Value::Null);
    let timing: Value = serde_json::from_str(&durable.recv_output().await.unwrap().text).unwrap();
    assert_eq!(timing["event"], "script_timing");
    assert_eq!(timing["succeeded"], true);
}

#[tokio::test]
async fn script_verifier_rejects_malformed_results_and_preserves_crashes() {
    let valid = json!({"output":{"result":"ok"},"signals":{"status":"ready"},"diagnostic":null});
    let mut invalid = vec![json!(null), json!({"response":valid}), json!([valid])];
    for field in ["output", "signals", "diagnostic"] {
        let mut value = valid.clone();
        value.as_object_mut().unwrap().remove(field);
        invalid.push(value);
    }
    for (field, replacement) in [
        ("extra", json!(null)),
        ("signals", json!({})),
        ("signals", json!({"status":"ready","extra":"ready"})),
        ("signals", json!({"status":"unknown"})),
        ("signals", json!({"status":42})),
        ("output", json!({"result":42})),
        ("diagnostic", json!("wrong type")),
    ] {
        let mut value = valid.clone();
        value[field] = replacement;
        invalid.push(value);
    }
    let mut cases = invalid
        .into_iter()
        .map(|value| {
            (
                format!("print({:?})\n", value.to_string()),
                WorkerErrorCode::Malformed,
            )
        })
        .collect::<Vec<_>>();
    cases.extend([
        ("print('not json')\n".to_owned(), WorkerErrorCode::Malformed),
        (
            format!("print({:?})\nsys.exit(7)\n", valid.to_string()),
            WorkerErrorCode::Crash,
        ),
    ]);
    for (source, expected) in cases {
        let directory = TestDirectory::new("script-verifier-invalid");
        let (adapter, invocation) = prepared(&directory, &source).await;
        let (outcome, _durable) = run_script(&adapter, verifier(invocation)).await;
        assert!(
            matches!(outcome, WorkerOutcome::Error { code, .. } if code == expected),
            "{source}: {outcome:?}"
        );
    }
}

#[tokio::test]
async fn script_exchanges_large_json_preserves_literal_arguments_and_redacts_stderr() {
    let directory = TestDirectory::new("script-json");
    let executable = directory.write_executable("worker", &python3_script(
        "#!/usr/bin/env python3\nimport json, os, sys\npayload = json.load(sys.stdin)\nassert os.environ['ZEROSHOT_NODE_NAME'] == 'script'\nassert os.environ['ZEROSHOT_NODE_KIND'] == 'step'\nassert os.environ['ZEROSHOT_NODE_ATTEMPT'] == '1'\nprint(os.environ['SCRIPT_TOKEN'], file=sys.stderr)\nprint(json.dumps({'result': payload['text'] + sys.argv[1]}))\n"
    ));
    let adapter = ScriptAdapter::new(directory.path().to_owned(), None);
    let literal = "$(touch must-not-exist); literal argument";
    let data = "x".repeat(512 * 1024);
    let mut invocation = invocation(
        &adapter,
        vec![
            executable.to_string_lossy().into_owned(),
            literal.to_owned(),
        ],
        json!({"text":data}),
    )
    .await;
    invocation.environment = invocation.environment.with_variables(&BTreeMap::from([(
        EnvironmentVariableName::new("PREFIX").unwrap(),
        "private-script-".to_owned(),
    )]));
    let (result, mut durable) = run_script(&adapter, invocation).await;
    let WorkerOutcome::Verified { output, .. } = result else {
        panic!("expected script success")
    };
    assert_eq!(output["result"], format!("{data}{literal}"));
    assert!(!directory.child("must-not-exist").exists());
    assert_eq!(
        durable.recv_usage().await.unwrap(),
        Some(crate::native_v2_contract::TokenUsageDelta {
            input_tokens: Default::default(),
            output_tokens: Default::default(),
            cache_read_input_tokens: Some(Default::default()),
            cache_creation_input_tokens: Some(Default::default()),
        })
    );
    let log = durable.recv_output().await.unwrap();
    assert!(!log.text.contains("private-script-fixture-key"));
    assert!(!log.text.contains("fixture-key"));
    assert!(log.text.contains("[REDACTED]"));
}

#[tokio::test]
async fn script_usage_preserves_agent_cache_totals_in_the_run_ledger() {
    use openengine_cluster_protocol::{IdempotencyKey, Sha256Digest, TokenCount, TokenUsage};
    use crate::native_v2_contract::TokenUsageDelta;
    use crate::native_v2_runner::test_support::{admitted, node_started, request};
    use crate::v2_run_ledger::{CreateRun, RunLedger, sqlite::SqliteRunLedger};

    let directory = TestDirectory::new("script-usage");
    let (adapter, invocation) = prepared(&directory, "print('{\"result\":\"ok\"}')\n").await;
    let script_reference = invocation.node.reference.clone();
    let run_id = script_reference.run_id.clone();
    let (outcome, mut durable) = run_script(&adapter, invocation).await;
    assert!(matches!(outcome, WorkerOutcome::Verified { .. }));
    let script_usage = durable.recv_usage().await.unwrap();
    let agent_reference = request(run_id.as_str(), "worker", (2, 2))
        .invocation
        .reference;
    let agent_usage = TokenUsageDelta {
        input_tokens: TokenCount::new(13).unwrap(),
        output_tokens: TokenCount::new(5).unwrap(),
        cache_read_input_tokens: Some(TokenCount::new(8).unwrap()),
        cache_creation_input_tokens: Some(TokenCount::new(3).unwrap()),
    };
    let ledger = SqliteRunLedger::open_in_memory().unwrap();
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("script-usage").unwrap(),
            submission_digest: Sha256Digest::new("a".repeat(64)).unwrap(),
            admitted: admitted(),
        })
        .await
        .unwrap();
    let stored = ledger
        .append(
            &run_id,
            vec![
                RunEvent::RunStarted,
                node_started(&agent_reference, Value::Null),
                RunEvent::TokenUsageObserved {
                    execution: agent_reference.execution,
                    usage: Some(agent_usage),
                },
                RunEvent::NodeCompleted {
                    completion: NodeCompletion {
                        reference: agent_reference,
                        outcome: WorkerOutcome::Verified {
                            output: Value::Null,
                            artifacts: vec![],
                        },
                    },
                },
                node_started(&script_reference, json!({})),
                RunEvent::TokenUsageObserved {
                    execution: script_reference.execution,
                    usage: script_usage,
                },
                RunEvent::NodeCompleted {
                    completion: NodeCompletion {
                        reference: script_reference,
                        outcome,
                    },
                },
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        stored.snapshot.token_usage,
        Some(TokenUsage {
            input_tokens: agent_usage.input_tokens,
            output_tokens: agent_usage.output_tokens,
            cache_read_input_tokens: agent_usage.cache_read_input_tokens,
            cache_creation_input_tokens: agent_usage.cache_creation_input_tokens,
            complete: true,
        })
    );
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
        let (result, _durable) = run_script(&adapter, invocation).await;
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
    for is_verifier in [false, true] {
        let directory = TestDirectory::new("script-cancel");
        let (adapter, invocation) = prepared(
            &directory,
            "import pathlib, time\npathlib.Path('started').write_text('ready')\ntime.sleep(60)\n",
        )
        .await;
        let invocation = if is_verifier {
            verifier(invocation)
        } else {
            invocation
        };
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
}

#[tokio::test]
async fn truncated_stderr_cannot_publish_a_credential_fragment() {
    let directory = TestDirectory::new("script-truncated-secret");
    let (adapter, invocation) = prepared(&directory,
        "import os\nprint(os.environ['SCRIPT_TOKEN'] + 'x' * (64 * 1024 - 8), file=sys.stderr)\nprint('{\"result\":\"ok\"}')\n").await;
    let (outcome, mut durable) = run_script(&adapter, invocation).await;
    assert!(matches!(outcome, WorkerOutcome::Verified { .. }));
    let diagnostic = durable.recv_output().await.unwrap();
    assert!(diagnostic.text.contains("diagnostics omitted"));
    assert!(!diagnostic.text.contains("ture-key"));
    assert!(!diagnostic.text.contains("xxxxxxxx"));
}

#[tokio::test]
async fn schema_diagnostics_redact_credentials_before_truncation() {
    let directory = TestDirectory::new("script-schema-secret");
    let (adapter, mut invocation) = prepared(
        &directory,
        "import json, os\nprint(json.dumps('x' * 8000 + os.environ['SCRIPT_TOKEN']))\n",
    )
    .await;
    invocation.response = NodeResponseContract::Worker {
        output: serde_json::from_value(json!({"kind":"enum", "values":["allowed"]})).unwrap(),
    };
    invocation.environment = ResolvedEnvironment::exact(
        &invocation.node.binding,
        BTreeMap::from([(
            EnvironmentVariableName::new("SCRIPT_TOKEN").unwrap(),
            "private-script-fixture-key".repeat(16),
        )]),
    )
    .unwrap();
    let (outcome, mut durable) = run_script(&adapter, invocation).await;
    assert!(matches!(
        outcome,
        WorkerOutcome::Error {
            code: WorkerErrorCode::Malformed,
            ..
        }
    ));
    let diagnostic = durable.recv_output().await.unwrap();
    assert!(diagnostic.text.contains("violates declared schema"));
    assert!(diagnostic.text.contains("[REDACTED]"));
    assert!(!diagnostic.text.contains("private-script-fixture"));
}

#[tokio::test]
async fn output_at_the_serialized_event_boundary_persists_or_becomes_malformed() {
    use openengine_cluster_protocol::{IdempotencyKey, Sha256Digest};
    use crate::native_v2_runner::test_support::{admitted, node_started};
    use crate::v2_run_ledger::{CreateRun, MAX_EVENT_BYTES, RunLedger, sqlite::SqliteRunLedger};

    for is_verifier in [false, true] {
        for excess in [0, 1, 256] {
            let directory = TestDirectory::new("script-event-size");
            let (adapter, invocation) = prepared(
                &directory,
                "import json\nprint(json.dumps({'result':'x' * 1}))\n",
            )
            .await;
            let reference = invocation.node.reference;
            let empty_event = RunEvent::NodeCompleted {
                completion: NodeCompletion {
                    reference: reference.clone(),
                    outcome: if is_verifier {
                        WorkerOutcome::Verifier {
                            output: json!({"result":""}),
                            signals: serde_json::from_value(json!({"status":"ready"})).unwrap(),
                            diagnostic: Value::Null,
                            artifacts: vec![],
                        }
                    } else {
                        WorkerOutcome::Verified {
                            output: json!({"result":""}),
                            artifacts: vec![],
                        }
                    },
                },
            };
            let envelope = serde_json::to_vec(&empty_event).unwrap().len();
            let response = if is_verifier {
                "{'output':payload,'signals':{'status':'ready'},'diagnostic':None}"
            } else {
                "payload"
            };
            let source = format!(
                "import json\npayload = {{'result':'x' * {}}}\nprint(json.dumps({response}))\n",
                MAX_EVENT_BYTES - envelope + excess,
            );
            let (_, invocation) = prepared(&directory, &source).await;
            let invocation = if is_verifier {
                verifier(invocation)
            } else {
                invocation
            };
            let (outcome, _durable) = run_script(&adapter, invocation).await;
            if excess == 0 {
                assert!(matches!(
                    outcome,
                    WorkerOutcome::Verified { .. } | WorkerOutcome::Verifier { .. }
                ));
            } else {
                assert!(matches!(
                    outcome,
                    WorkerOutcome::Error {
                        code: WorkerErrorCode::Malformed,
                        ..
                    }
                ));
            }
            let ledger = SqliteRunLedger::open_in_memory().unwrap();
            ledger
                .create_or_get(CreateRun {
                    run_id: reference.run_id.clone(),
                    submission_key: IdempotencyKey::new("script-size").unwrap(),
                    submission_digest: Sha256Digest::new("a".repeat(64)).unwrap(),
                    admitted: admitted(),
                })
                .await
                .unwrap();
            let expected = outcome.clone();
            let stored = ledger
                .append(
                    &reference.run_id,
                    vec![
                        RunEvent::RunStarted,
                        node_started(&reference, json!({})),
                        RunEvent::NodeCompleted {
                            completion: NodeCompletion {
                                reference: reference.clone(),
                                outcome,
                            },
                        },
                    ],
                )
                .await
                .unwrap();
            assert_eq!(
                stored.snapshot.executions[&reference.execution].outcome(),
                Some(&expected)
            );
        }
    }
}
