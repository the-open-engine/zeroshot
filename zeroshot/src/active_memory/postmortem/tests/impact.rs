use super::*;

use openengine_cluster_protocol::{
    Cursor, NodeName, PositiveInteger, RunId, TerminalResult, WorkerFailureReason,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use crate::full_v1_reducer::{
    DurableExecution, DurableExecutionState, ExecutionVoidReason, HistoryPosition,
    StructuralOccurrence,
};
use crate::native_v2_admission::NativeV2Admission;
use crate::native_v2_contract::{AdmittedRun, ExecutionRef, NodeInstanceId, RunSubmission};
use crate::v2_run_ledger::{RunSnapshot, cursor_for};
use crate::v2_run_ledger::fake::FakeRunLedger;

struct Case {
    admitted: AdmittedRun,
    snapshot: RunSnapshot,
    terminal: TerminalResult,
    ledger: FakeRunLedger,
    instances: BTreeMap<(String, Vec<u64>), NodeInstanceId>,
}

impl Case {
    async fn new(body: Value) -> Self {
        let root = sequence(
            "root",
            vec![
                body,
                json!({
                    "kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]
                }),
            ],
        );
        let graph: GraphNode = serde_json::from_value(root.clone()).assert_value();
        let mut bindings = serde_json::Map::new();
        collect_bindings(&graph, &mut bindings);
        let submission: RunSubmission = serde_json::from_value(json!({
            "title":"Review impact test","submissionKey":"impact-test",
            "initialInput":{"items":[null,null]},
            "graph":{"profile":"openengine.graph.full/v1","initialInput":state(),
                "policy":{"policy":"policy.native-v2@1","default":"deny"},"root":root},
            "runtime":{"harness":"codex","provider":"openai","size":"small","nodes":bindings}
        }))
        .assert_value();
        let admitted = NativeV2Admission
            .admit(submission)
            .await
            .unwrap_or_else(|error| panic!("review impact fixture admission failed: {error:?}"));
        Self {
            snapshot: RunSnapshot::admitted(RunId::new("impact-test-run"), &admitted),
            admitted,
            terminal: TerminalResult::Succeeded {
                output: Value::Null,
            },
            ledger: FakeRunLedger::new(),
            instances: BTreeMap::new(),
        }
    }

    fn visit(&mut self, name: &str, outcome: WorkerOutcome, scope: &[u64]) -> ExecutionId {
        let number = self.snapshot.executions.len() as u64 + 1;
        let execution = ExecutionId::new(number).assert_value();
        let next_instance = NodeInstanceId::new(self.instances.len() as u64 + 1).assert_value();
        let instance = *self
            .instances
            .entry((name.to_owned(), scope.to_vec()))
            .or_insert(next_instance);
        let name: NodeName = name.parse().assert_value();
        let node = NodeSnapshot {
            reference: ExecutionRef {
                run_id: self.snapshot.run_id.clone(),
                node: name.clone(),
                node_instance: instance,
                execution,
            },
            occurrence: StructuralOccurrence {
                node: name,
                map_indices: scope.to_vec(),
            },
            attempt: PositiveInteger::new(1).assert_value(),
            failure_diagnostic: false,
            input: Value::Null,
            started_at: cursor_for(number * 3),
            state: NodeState::Completed {
                at: cursor_for(number * 3 + 1),
                outcome,
            },
        };
        self.snapshot.executions.insert(execution, node);
        execution
    }

    fn round(&mut self, workers: &[&str], reviews: &[(&str, &str)], scope: &[u64]) -> Vec<String> {
        for worker in workers {
            self.visit(worker, verified(), scope);
        }
        reviews
            .iter()
            .map(|(name, verdict)| {
                let execution = self.visit(name, reviewed(verdict), scope);
                completion_reference(&self.snapshot.executions[&execution]).assert_value()
            })
            .collect()
    }

    fn standard_rounds(&mut self, verdicts: &[&str], scope: &[u64]) -> Vec<String> {
        verdicts
            .iter()
            .filter_map(|verdict| {
                let references = self.round(
                    &["worker"],
                    &[("acceptance", verdict), ("code", "accepted")],
                    scope,
                );
                (*verdict == "rejected").then(|| references[0].clone())
            })
            .collect()
    }

    fn impact(&self) -> ImpactEvidence {
        ImpactEvidence::new(
            MainRunFacts {
                run_id: &self.snapshot.run_id,
                admitted: &self.admitted,
                snapshot: &self.snapshot,
                primary_terminal: &self.terminal,
                ledger: &self.ledger,
            },
            Vec::new(),
        )
    }
}

fn collect_bindings(node: &GraphNode, bindings: &mut serde_json::Map<String, Value>) {
    if matches!(node, GraphNode::Step(_) | GraphNode::Verifier(_)) {
        bindings.insert(node.name().as_str().to_owned(), json!({
            "kind":"agent","model":"opaque-test-model","connections":{},"sessionScope":"node_instance"
        }));
    }
    for child in openengine_cluster_server::graph_verifier::graph_node_children(node) {
        collect_bindings(child, bindings);
    }
}

async fn repair_case() -> Case {
    let mut case = Case::new(review_loop()).await;
    case.standard_rounds(&["rejected", "rejected", "accepted"], &[]);
    case
}

fn assert_repair_abstention(mut case: Case) {
    case.standard_rounds(&["rejected", "rejected", "accepted"], &[]);
    assert!(case.impact().episodes.is_empty());
}

fn state() -> Value {
    let array = json!({"kind":"array","items":{"kind":"null"}});
    let fields = BTreeMap::from([("items", json!({"type":array,"required":true}))]);
    json!({"kind":"record","fields":fields})
}

fn worker(name: &str) -> Value {
    json!({"kind":"step","name":name,"worker":"worker.test@1","instructions":"Implement the task.",
        "input":{"kind":"null"},"output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1})
}

fn reviewer(name: &str) -> Value {
    json!({"kind":"verifier","name":name,"worker":"verifier.test@1","instructions":"Review the task.",
        "input":{"kind":"null"},"output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1,
        "signals":{"verdict":["accepted","rejected"]},"diagnostic":{"kind":"null"}})
}

fn sequence(name: &str, children: Vec<Value>) -> Value {
    json!({"kind":"seq","name":name,"state":state(),"children":children,"promotedStatePaths":[]})
}

fn parallel(name: &str, branches: Vec<Value>) -> Value {
    json!({"kind":"par","name":name,"state":state(),"branches":branches,
        "join":{"kind":"all"},"promotedStatePaths":[]})
}

fn looped(name: &str, body: Value, until: Option<Value>, maximum: u64) -> Value {
    json!({"kind":"loop","name":name,"state":state(),"body":body,"until":until,
        "maxIterations":maximum,"promotedStatePaths":[]})
}

fn control(name: &str, source: ControlSource, labels: &[&str]) -> Value {
    let field = (source == ControlSource::Signal).then_some("verdict");
    json!({"kind":"in","value":{"name":name,"source":source,"field":field},"labels":labels})
}

fn accepted(name: &str) -> Value {
    control(name, ControlSource::Signal, &["accepted"])
}

fn error(name: &str) -> Value {
    control(
        name,
        ControlSource::Error,
        &["crash", "malformed", "refusal", "timeout"],
    )
}

fn mapped(body: Value) -> Value {
    json!({"kind":"map","name":"items","state":state(),"body":body,
        "over":{"source":"state","path":["items"]},"maxItems":2,"promotedStatePaths":[]})
}

fn until(reviewers: &[&str]) -> Value {
    json!({"kind":"any","guards":[
        {"kind":"all","guards":reviewers.iter().map(|name| accepted(name)).collect::<Vec<_>>()},
        {"kind":"any","guards":reviewers.iter().map(|name| error(name)).collect::<Vec<_>>()}
    ]})
}

fn error_exit(name: &str, source: &str, otherwise: Value) -> Value {
    json!({"kind":"choice","name":name,"state":state(),"branches":[{
        "when":error(source),"node":{"kind":"fail","name":format!("{name}_failed"),"reason":"test_failed"}
    }],"otherwise":otherwise,"promotedStatePaths":[]})
}

fn review_body() -> Value {
    sequence(
        "iteration",
        vec![
            worker("worker"),
            parallel("reviews", vec![reviewer("acceptance"), reviewer("code")]),
        ],
    )
}

fn review_loop() -> Value {
    looped(
        "review_loop",
        review_body(),
        Some(until(&["acceptance", "code"])),
        10,
    )
}

fn verified() -> WorkerOutcome {
    WorkerOutcome::Verified {
        output: Value::Null,
        artifacts: Vec::new(),
    }
}

fn reviewed(verdict: &str) -> WorkerOutcome {
    WorkerOutcome::Verifier {
        output: Value::Null,
        signals: BTreeMap::from([(
            "verdict".parse().assert_value(),
            verdict.parse().assert_value(),
        )]),
        diagnostic: Value::Null,
        artifacts: Vec::new(),
    }
}

fn assert_worker_references(evidence: &ImpactEvidence, references: &[String]) {
    let episode = evidence.episodes.get("worker").assert_value();
    assert_eq!(
        episode.failure_references,
        references.iter().cloned().collect()
    );
    for role in [Role::Graph, Role::Repository] {
        assert!(episode.allows(role, references));
    }
}

#[tokio::test]
async fn standard_error_exit_review_loops_attribute_distinct_repair_rounds_to_the_worker() {
    let tail = parallel(
        "reviews",
        vec![
            reviewer("acceptance"),
            reviewer("code"),
            sequence(
                "testing",
                vec![
                    reviewer("tests"),
                    error_exit("test_result", "tests", reviewer("adversary")),
                ],
            ),
        ],
    );
    let body = sequence(
        "iteration",
        vec![
            worker("worker"),
            error_exit("worker_result", "worker", tail),
        ],
    );
    let mut case = Case::new(looped(
        "review_loop",
        body,
        Some(until(&["acceptance", "code", "tests", "adversary"])),
        10,
    ))
    .await;
    let mut anchors = Vec::new();
    for verdict in ["rejected", "rejected", "accepted"] {
        let references = case.round(
            &["worker"],
            &[
                ("acceptance", verdict),
                ("code", "accepted"),
                ("tests", "accepted"),
                ("adversary", "accepted"),
            ],
            &[],
        );
        if verdict == "rejected" {
            anchors.push(references[0].clone());
        }
    }
    let evidence = case.impact();
    assert_worker_references(&evidence, &anchors);
    assert_eq!(evidence.episodes.len(), 1);
    assert!(evidence.tools.is_empty());
}

#[tokio::test]
async fn two_rejected_reviewers_in_one_round_do_not_count_as_two_repairs() {
    let mut case = Case::new(review_loop()).await;
    case.round(
        &["worker"],
        &[("acceptance", "rejected"), ("code", "rejected")],
        &[],
    );
    case.round(
        &["worker"],
        &[("acceptance", "accepted"), ("code", "accepted")],
        &[],
    );
    assert!(case.impact().episodes.is_empty());
}

#[tokio::test]
async fn successful_inner_loops_repeated_by_an_outer_loop_are_not_rejections() {
    let mut case = Case::new(looped("outer", review_loop(), None, 3)).await;
    case.standard_rounds(&["accepted", "accepted", "accepted"], &[]);
    assert!(case.impact().episodes.is_empty());
}

#[tokio::test]
async fn failed_active_voided_and_retried_visits_abstain_from_review_attribution() {
    let states = [
        NodeState::Active,
        NodeState::Voided {
            at: cursor_for(100),
            reason: ExecutionVoidReason::ParallelJoin,
        },
        NodeState::Completed {
            at: cursor_for(100),
            outcome: WorkerOutcome::Error {
                code: WorkerErrorCode::Malformed,
                reason: WorkerFailureReason::MalformedResult,
            },
        },
        NodeState::Completed {
            at: cursor_for(100),
            outcome: WorkerOutcome::Error {
                code: WorkerErrorCode::Crash,
                reason: WorkerFailureReason::DeclaredFailure,
            },
        },
    ];
    for state in states {
        let mut case = repair_case().await;
        let changed = case
            .snapshot
            .executions
            .get_mut(&ExecutionId::new(3).assert_value())
            .assert_value();
        changed.state = state;
        let evidence = case.impact();
        assert!(!evidence.episodes.contains_key("worker"));
        if matches!(
            case.snapshot.executions[&ExecutionId::new(3).assert_value()].outcome(),
            Some(WorkerOutcome::Error {
                code: WorkerErrorCode::Crash,
                ..
            })
        ) {
            assert_eq!(
                evidence.episodes["code"].failure_references,
                BTreeSet::from(["v2:100".to_owned()])
            );
        }
    }
    let mut case = repair_case().await;
    case.snapshot
        .executions
        .get_mut(&ExecutionId::new(3).assert_value())
        .assert_value()
        .attempt = PositiveInteger::new(2).assert_value();
    assert!(case.impact().episodes.is_empty());
}

#[tokio::test]
async fn a_false_error_guard_cannot_masquerade_as_a_rejected_review() {
    for guard in [
        error("acceptance"),
        json!({"kind":"all","guards":[accepted("acceptance"), error("code")]}),
    ] {
        let mut case = Case::new(looped("review_loop", review_body(), Some(guard), 10)).await;
        case.standard_rounds(&["accepted", "accepted", "accepted"], &[]);
        assert!(case.impact().episodes.is_empty());
    }
}

#[tokio::test]
async fn unobserved_error_exit_siblings_remain_unknown() {
    let guard = json!({"kind":"any","guards":[until(&["acceptance", "code"]), error("outside")]});
    let body = sequence(
        "iteration",
        vec![
            worker("worker"),
            parallel(
                "reviews",
                vec![
                    reviewer("acceptance"),
                    reviewer("code"),
                    reviewer("outside"),
                ],
            ),
        ],
    );
    let case = Case::new(looped("review_loop", body, Some(guard), 10)).await;
    assert_repair_abstention(case);
}

#[tokio::test]
async fn distinct_map_items_cannot_pool_rejections_to_reach_the_cost_threshold() {
    let mut case = Case::new(mapped(review_loop())).await;
    for scope in [&[0][..], &[1][..]] {
        case.standard_rounds(&["rejected", "accepted"], scope);
    }
    assert!(case.impact().episodes.is_empty());
}

#[tokio::test]
async fn eligible_map_item_references_do_not_include_another_items_verdicts() {
    let mut case = Case::new(mapped(review_loop())).await;
    let mut anchors = Vec::new();
    for verdict in ["rejected", "rejected", "accepted"] {
        anchors.extend(case.standard_rounds(&[verdict], &[0]));
        case.standard_rounds(&["accepted"], &[1]);
    }
    assert_worker_references(&case.impact(), &anchors);
}

#[tokio::test]
async fn a_failed_reviewer_in_another_map_item_prevents_global_error_absence() {
    let mut case = Case::new(mapped(review_loop())).await;
    case.standard_rounds(&["rejected", "rejected", "accepted"], &[0]);
    case.visit("worker", verified(), &[1]);
    case.visit("acceptance", reviewed("accepted"), &[1]);
    case.visit(
        "code",
        WorkerOutcome::Error {
            code: WorkerErrorCode::Malformed,
            reason: WorkerFailureReason::MalformedResult,
        },
        &[1],
    );
    assert!(case.impact().episodes.is_empty());
}

#[test]
fn review_branches_without_a_full_join_are_outside_the_supported_shape() {
    let mut body = review_body();
    body["children"][1]["join"] = json!({"kind":"any"});
    let graph: GraphNode = serde_json::from_value(body).assert_value();
    assert!(ReviewLoop::new(&graph, &BTreeSet::new()).is_none());
}

#[tokio::test]
async fn nested_intermediate_rejections_do_not_become_outer_worker_evidence() {
    let body = sequence(
        "outer_iteration",
        vec![
            worker("outer_worker"),
            review_loop(),
            reviewer("outer_check"),
        ],
    );
    let mut case = Case::new(looped(
        "outer",
        body,
        Some(until(&["acceptance", "outer_check"])),
        2,
    ))
    .await;
    let mut anchors = Vec::new();
    for _ in 0..2 {
        case.visit("outer_worker", verified(), &[]);
        anchors.extend(case.standard_rounds(&["rejected", "rejected", "accepted"], &[]));
        case.visit("outer_check", reviewed("rejected"), &[]);
    }
    let evidence = case.impact();
    assert_worker_references(&evidence, &anchors);
    assert_eq!(evidence.episodes.len(), 1);
}

#[tokio::test]
async fn exhausted_final_rounds_cannot_borrow_successors_from_outer_reentry() {
    let inner = looped(
        "review_loop",
        review_body(),
        Some(until(&["acceptance", "code"])),
        1,
    );
    let mut case = Case::new(looped("outer", inner, None, 3)).await;
    case.standard_rounds(&["rejected", "rejected", "rejected"], &[]);
    assert!(case.impact().episodes.is_empty());
}

#[tokio::test]
async fn only_serial_prefix_workers_receive_review_cost() {
    let body = sequence(
        "iteration",
        vec![
            worker("worker"),
            worker("helper"),
            parallel("reviews", vec![reviewer("acceptance"), reviewer("code")]),
        ],
    );
    let work = looped(
        "review_loop",
        body,
        Some(until(&["acceptance", "code"])),
        10,
    );
    let unrelated = looped("unrelated_loop", worker("unrelated"), None, 2);
    let mut case = Case::new(parallel("branches", vec![work, unrelated])).await;
    let mut anchors = Vec::new();
    for verdict in ["rejected", "rejected", "accepted"] {
        let references = case.round(
            &["worker", "helper"],
            &[("acceptance", verdict), ("code", "accepted")],
            &[],
        );
        if verdict == "rejected" {
            anchors.push(references[0].clone());
        }
    }
    for _ in 0..2 {
        case.visit("unrelated", verified(), &[]);
    }
    let evidence = case.impact();
    assert_worker_references(&evidence, &anchors);
    assert_eq!(
        evidence.episodes["helper"].failure_references,
        anchors.into_iter().collect()
    );
    assert_eq!(evidence.episodes.len(), 2);
}

#[tokio::test]
async fn seeded_history_cannot_prove_complete_rounds_or_error_absence() {
    let mut case = repair_case().await;
    let node = case.snapshot.executions.values().next().assert_value();
    case.snapshot.execution_seed.push(DurableExecution {
        dispatch_position: HistoryPosition::new(1).assert_value(),
        node_instance: node.reference.node_instance,
        execution: node.reference.execution,
        occurrence: node.occurrence.clone(),
        attempt: node.attempt,
        input: node.input.clone(),
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(2).assert_value(),
            outcome: verified(),
        },
    });
    assert!(case.impact().episodes.is_empty());
}

#[tokio::test]
async fn incomplete_or_overlapping_rounds_cannot_prove_review_repairs() {
    for overlap in [false, true] {
        let mut case = repair_case().await;
        if overlap {
            case.snapshot
                .executions
                .get_mut(&ExecutionId::new(3).assert_value())
                .assert_value()
                .state = NodeState::Completed {
                at: Cursor::new("v2:100"),
                outcome: reviewed("accepted"),
            };
        } else {
            case.snapshot
                .executions
                .remove(&ExecutionId::new(3).assert_value());
        }
        assert!(case.impact().episodes.is_empty());
    }
}

#[tokio::test]
async fn repeated_response_format_failures_and_latency_do_not_create_review_cost() {
    let mut case = Case::new(review_loop()).await;
    for _ in 0..2 {
        case.visit(
            "worker",
            WorkerOutcome::Error {
                code: WorkerErrorCode::Malformed,
                reason: WorkerFailureReason::MalformedResult,
            },
            &[],
        );
    }
    assert!(case.impact().episodes.is_empty());
    case.snapshot.executions.clear();
    case.standard_rounds(&["accepted", "accepted", "accepted"], &[]);
    for node in case.snapshot.executions.values_mut() {
        node.started_at = cursor_for(cursor_sequence(&node.started_at).assert_value() * 1_000_000);
        if let NodeState::Completed { at, .. } = &mut node.state {
            *at = cursor_for(cursor_sequence(at).assert_value() * 1_000_000);
        }
    }
    let evidence = case.impact();
    assert!(evidence.episodes.is_empty());
    assert!(evidence.tools.is_empty());
}

#[test]
fn exploration_cost_must_be_proved_by_the_cited_tool_receipts() {
    let mut episode = ImpactEpisode::default();
    for (reference, time) in [
        ("hook:1", 1),
        ("hook:2", 100),
        ("hook:3", 200),
        ("hook:4", 300001),
    ] {
        episode.tool_references.insert(reference.to_owned());
        episode.tool_timestamps.insert(reference.to_owned(), time);
        episode.time(time);
    }
    let references = |values: [&str; 3]| values.into_iter().map(str::to_owned).collect::<Vec<_>>();
    assert!(!episode.allows(
        Role::Repository,
        &references(["hook:1", "hook:2", "hook:3"])
    ));
    assert!(episode.allows(
        Role::Repository,
        &references(["hook:1", "hook:2", "hook:4"])
    ));
    assert!(!episode.allows(Role::Graph, &references(["hook:1", "hook:2", "hook:4"])));
}
