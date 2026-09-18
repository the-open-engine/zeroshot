use super::*;
use crate::full_v1_reducer::{Decision, FullV1Reducer, ReductionInput};
use crate::native_v2_admission::NativeV2Admission;
use crate::native_v2_supervisor::{durable_history, next_execution, next_node_instance};
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
use openengine_cluster_protocol::{RunSubmission, NodeRuntimeBinding};
use openengine_cluster_server::admission::VerifiedGraph;
use std::collections::BTreeMap;

fn terminal() -> RunEvent {
    RunEvent::Terminal {
        result: TerminalResult::Succeeded {
            output: Value::Null,
        },
    }
}

async fn software_fixture() -> Fixture {
    let graph = BuiltinGraphTemplate::SoftwareChange
        .materialize(TemplateDelivery::None)
        .assert_value();
    let runtime = crate::native_v2_contract::RuntimePlan::Codex {
        provider: crate::native_v2_contract::CodexProvider::OpenAi,
        size: RunSize::Small,
        nodes: ["worker", "acceptance", "code", "review_repair"]
            .into_iter()
            .map(|name| {
                (
                    NodeName::new(name).assert_value(),
                    serde_json::from_value::<NodeRuntimeBinding>(
                        json!({"kind":"agent","model":"test-model","sessionScope":"execution"}),
                    )
                    .assert_value(),
                )
            })
            .collect(),
    };
    let initial_input = json!({"task":"Implement atomic quota reservation with regression tests"});
    let admitted = NativeV2Admission
        .admit(RunSubmission {
            title: RunTitle::new("Inspect control flow").assert_value(),
            graph,
            initial_input: initial_input.clone(),
            runtime,
            source: ResolvedSource {
                repository: SourceRepositoryId::new("open-engine/test-matrix").assert_value(),
                branch: SourceBranchId::new("main").assert_value(),
                revision: SourceRevisionId::new("a".repeat(40)).assert_value(),
            },
            submission_key: IdempotencyKey::new("structural-history").assert_value(),
        })
        .await
        .assert_value();
    Fixture::with_graph(admitted.graph, initial_input).await
}

async fn dispatch_ready(fixture: &Fixture) -> Vec<ExecutionRef> {
    let stored = fixture
        .ledger
        .get(&fixture.id)
        .await
        .assert_value()
        .assert_value();
    let executions = durable_history(&stored.snapshot).assert_value();
    let graph = VerifiedGraph {
        compiled_ir: stored.admitted.graph,
        diagnostics: Vec::new(),
    };
    let reduction = FullV1Reducer::native_v2(&graph)
        .reduce(ReductionInput {
            initial_input: &stored.admitted.initial_input,
            executions: &executions,
            next_node_instance: next_node_instance(&executions).assert_value(),
            next_execution: next_execution(&executions).assert_value(),
        })
        .assert_value();
    let mut references = Vec::new();
    let events = reduction
        .decisions
        .into_iter()
        .filter_map(|decision| {
            let Decision::Dispatch {
                node_instance,
                execution,
                occurrence,
                attempt,
                input,
                ..
            } = decision
            else {
                return None;
            };
            let reference = ExecutionRef {
                run_id: fixture.id.clone(),
                node: occurrence.node.clone(),
                node_instance,
                execution,
            };
            references.push(reference.clone());
            Some(RunEvent::NodeStarted {
                reference,
                occurrence,
                attempt,
                input,
            })
        })
        .collect::<Vec<_>>();
    fixture
        .ledger
        .append(&fixture.id, events)
        .await
        .assert_value();
    references
}

async fn finish(fixture: &Fixture, reference: ExecutionRef, verdict: Option<&str>) {
    let outcome = match verdict {
        Some(verdict) => WorkerOutcome::Verifier {
            output: Value::Null,
            signals: BTreeMap::from([(
                "verdict".parse().assert_value(),
                verdict.parse().assert_value(),
            )]),
            diagnostic: json!({"message":"Inspected requirements and regression coverage."}),
            artifacts: Vec::new(),
        },
        None => WorkerOutcome::Verified {
            output: Value::Null,
            artifacts: Vec::new(),
        },
    };
    fixture
        .ledger
        .append(
            &fixture.id,
            vec![RunEvent::NodeCompleted {
                completion: NodeCompletion { reference, outcome },
            }],
        )
        .await
        .assert_value();
}

async fn begin_reviews(fixture: &Fixture) -> Vec<ExecutionRef> {
    fixture
        .ledger
        .append(&fixture.id, vec![RunEvent::RunStarted])
        .await
        .assert_value();
    let workers = dispatch_ready(fixture).await;
    finish(fixture, workers[0].clone(), None).await;
    dispatch_ready(fixture).await
}

fn controls(page: &Value) -> Vec<Value> {
    page["control"].as_array().cloned().unwrap_or_default()
}

#[tokio::test]
async fn software_change_choice_and_early_finish_have_exact_prefix_control_activity() {
    let fixture = software_fixture().await;
    let reviews = begin_reviews(&fixture).await;
    finish(&fixture, reviews[0].clone(), Some("accepted")).await;
    let before = fixture.service.page(&fixture.id, None).await.assert_value();
    assert!(before.get("controlError").is_none(), "{before}");
    assert!(
        !controls(&before)
            .iter()
            .any(|record| record["node"] == "review_result" || record["node"] == "done")
    );
    let cursor = before["nextCursor"].as_str().assert_value().to_owned();
    finish(&fixture, reviews[1].clone(), Some("accepted")).await;
    fixture
        .ledger
        .append(&fixture.id, vec![terminal()])
        .await
        .assert_value();
    let after = fixture
        .service
        .page(&fixture.id, Some(Cursor::new(cursor)))
        .await
        .assert_value();
    assert!(after.get("controlError").is_none(), "{after}");
    let records = controls(&after);
    assert!(
        records
            .iter()
            .any(|record| record["node"] == "review_result"
                && record["branch"] == "done"
                && record["state"] == "completed")
    );
    assert!(records.iter().any(|record| record["node"] == "done"
        && record["state"] == "succeeded"
        && record.get("output") == Some(&Value::Null)));
    let cursors = after["events"]
        .as_array()
        .assert_value()
        .iter()
        .map(|event| event["cursor"].clone())
        .collect::<Vec<_>>();
    assert!(
        records
            .iter()
            .all(|record| cursors.contains(&record["cursor"]))
    );
    // A later cached projection may never leak a future decision into an earlier page.
    let cached = fixture
        .service
        .control
        .project(&fixture.ledger, &fixture.id, 0..6)
        .await;
    assert!(
        cached
            .records
            .iter()
            .all(|record| record.node != "done" && record.node != "review_result")
    );
}

#[tokio::test]
async fn repeated_reviews_keep_distinct_choice_visits_and_sse_reconnect_matches_backfill() {
    let fixture = software_fixture().await;
    let reviews = begin_reviews(&fixture).await;
    finish(&fixture, reviews[0].clone(), Some("rejected")).await;
    finish(&fixture, reviews[1].clone(), Some("accepted")).await;
    let first = fixture.service.page(&fixture.id, None).await.assert_value();
    let repair = dispatch_ready(&fixture).await;
    finish(&fixture, repair[0].clone(), None).await;
    let reviews = dispatch_ready(&fixture).await;
    for review in reviews {
        finish(&fixture, review, Some("accepted")).await;
    }
    fixture
        .ledger
        .append(&fixture.id, vec![terminal()])
        .await
        .assert_value();
    let full = fixture.service.page(&fixture.id, None).await.assert_value();
    let routes = controls(&full)
        .into_iter()
        .filter(|record| record["node"] == "review_result")
        .collect::<Vec<_>>();
    let repair_visit = routes
        .iter()
        .find(|record| record["branch"] == "review_repair")
        .assert_value();
    let done_visit = routes
        .iter()
        .find(|record| record["branch"] == "done")
        .assert_value();
    assert_ne!(repair_visit["visitId"], done_visit["visitId"]);
    let after = Cursor::new(first["nextCursor"].as_str().assert_value());
    let expected = fixture
        .service
        .page(&fixture.id, Some(after.clone()))
        .await
        .assert_value();
    // A fresh observer has no cache and must reconstruct the same prefix before resuming.
    let fresh = NativeRunHistory::new(fixture.root.clone());
    let stream = fresh
        .subscribe(fixture.id.clone(), Some(after))
        .await
        .assert_value();
    futures_util::pin_mut!(stream);
    let observed =
        serde_json::to_value(stream.next().await.assert_value().assert_value()).assert_value();
    assert_eq!(observed["control"], expected["control"]);
    assert_eq!(observed["events"], expected["events"]);
}

#[tokio::test]
async fn projection_failure_preserves_original_history_and_provider_output() {
    let fixture = Fixture::new().await;
    fixture.start(1).await; // This intentionally mismatches the retained graph's worker contract.
    let page = fixture.service.page(&fixture.id, None).await.assert_value();
    assert_eq!(page["controlError"], "control_projection_unavailable");
    assert_eq!(page["events"].as_array().assert_value().len(), 2);
    assert_eq!(page["events"][1]["event"]["reference"]["node"], "worker");
    let resumed = fixture
        .service
        .page(&fixture.id, Some(Cursor::new("v2:2")))
        .await
        .assert_value();
    assert_eq!(resumed["controlError"], page["controlError"]);
    assert!(controls(&resumed).is_empty());
}

#[tokio::test]
async fn stopped_runs_settle_pending_controls_without_selecting_post_stop_routes() {
    let fixture = software_fixture().await;
    let reviews = begin_reviews(&fixture).await;
    fixture
        .ledger
        .append(&fixture.id, vec![RunEvent::ForceStopRequested])
        .await
        .assert_value();
    for review in reviews {
        finish(&fixture, review, Some("accepted")).await;
    }
    fixture
        .ledger
        .append(
            &fixture.id,
            vec![RunEvent::Terminal {
                result: TerminalResult::Failed {
                    reason: "runtime_failed".parse().assert_value(),
                },
            }],
        )
        .await
        .assert_value();
    let page = fixture.service.page(&fixture.id, None).await.assert_value();
    assert!(page.get("controlError").is_none(), "{page}");
    let mut latest = BTreeMap::new();
    for record in controls(&page) {
        latest.insert(record["visitId"].as_str().assert_value().to_owned(), record);
    }
    assert!(latest.values().all(|record| record["state"] != "entered"));
    assert!(
        latest
            .values()
            .any(|record| record["node"] == "change_loop" && record["state"] == "stopped")
    );
    assert!(
        !latest
            .values()
            .any(|record| record["node"] == "review_result" || record["node"] == "done")
    );
}

#[tokio::test]
async fn bounded_pages_and_new_live_readers_publish_identical_control_deltas() {
    let fixture = software_fixture().await;
    let reviews = begin_reviews(&fixture).await;
    let logs = (0..MAX_REPLAY_EVENTS + 7)
        .map(|index| RunEvent::SafeLog {
            execution: Some(reviews[0].execution),
            timestamp: UnixTimestampMillis::new(index as u64 + 1).assert_value(),
            stream: SafeLogStream::Output,
            line: SafeLogLine::new(format!("Reviewing regression case {index}")).assert_value(),
        })
        .collect();
    fixture
        .ledger
        .append(&fixture.id, logs)
        .await
        .assert_value();
    for review in reviews {
        finish(&fixture, review, Some("accepted")).await;
    }
    fixture
        .ledger
        .append(&fixture.id, vec![terminal()])
        .await
        .assert_value();
    let first = fixture.service.page(&fixture.id, None).await.assert_value();
    assert_eq!(
        first["events"].as_array().assert_value().len(),
        MAX_REPLAY_EVENTS
    );
    assert_eq!(first["complete"], false);
    assert!(
        !controls(&first)
            .iter()
            .any(|record| record["node"] == "done")
    );
    let after = Cursor::new(first["nextCursor"].as_str().assert_value());
    let second = fixture
        .service
        .page(&fixture.id, Some(after.clone()))
        .await
        .assert_value();
    assert!(
        controls(&second)
            .iter()
            .any(|record| record["node"] == "done")
    );
    let fresh = NativeRunHistory::new(fixture.root.clone());
    let stream = fresh
        .subscribe(fixture.id.clone(), Some(after))
        .await
        .assert_value();
    futures_util::pin_mut!(stream);
    let streamed =
        serde_json::to_value(stream.next().await.assert_value().assert_value()).assert_value();
    assert_eq!(streamed, second);
    let again = fixture.service.page(&fixture.id, None).await.assert_value();
    assert_eq!(again, first);
}
