use super::*;

use openengine_cluster_protocol::{
    ControlSource, GraphNode, NodeName, PositiveInteger, RunId, TerminalResult, WorkerErrorCode,
    WorkerFailureReason, WorkerOutcome,
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
            snapshot: RunSnapshot::admitted(RunId::new("impact-test-run"), &admitted)
                .assert_value(),
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
    crate::active_memory::postmortem::tests::review_node(name)
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

fn reference(case: &Case, execution: ExecutionId) -> String {
    completion_reference(&case.snapshot.executions[&execution]).assert_value()
}

fn observation(seq: u64, node: &str, execution: ExecutionId) -> ToolObservation {
    ToolObservation {
        seq,
        node_id: node.to_owned(),
        execution_id: execution.get(),
        timestamp_ms: seq.max(1),
        tool_name: "read_file".to_owned(),
        paths: vec!["src/provider.rs".to_owned()],
    }
}

fn with_tools(case: &Case, tools: Vec<ToolObservation>) -> ImpactEvidence {
    ImpactEvidence::new(
        MainRunFacts {
            run_id: &case.snapshot.run_id,
            admitted: &case.admitted,
            snapshot: &case.snapshot,
            primary_terminal: &case.terminal,
            ledger: &case.ledger,
        },
        tools,
    )
}

#[tokio::test]
async fn every_observed_completion_is_evidence_without_outcome_or_count_thresholds() {
    for outcome in [
        verified(),
        reviewed("accepted"),
        reviewed("rejected"),
        WorkerOutcome::Error {
            code: WorkerErrorCode::Crash,
            reason: WorkerFailureReason::DeclaredFailure,
        },
        WorkerOutcome::Error {
            code: WorkerErrorCode::Timeout,
            reason: WorkerFailureReason::DeclaredFailure,
        },
        WorkerOutcome::Error {
            code: WorkerErrorCode::Malformed,
            reason: WorkerFailureReason::MalformedResult,
        },
        WorkerOutcome::Error {
            code: WorkerErrorCode::Refusal,
            reason: WorkerFailureReason::DeclaredFailure,
        },
    ] {
        let mut case = Case::new(review_loop()).await;
        let name = if matches!(outcome, WorkerOutcome::Verifier { .. }) {
            "acceptance"
        } else {
            "worker"
        };
        let execution = case.visit(name, outcome, &[]);
        let own = reference(&case, execution);
        let evidence = case.impact();
        let episode = &evidence.episodes[name];
        assert_eq!(episode.completion_references, BTreeSet::from([own.clone()]));
        assert!(episode.allows(std::slice::from_ref(&own)));
        assert!(evidence.references().contains(&own));
    }
}

fn retry_guard(name: &str) -> Value {
    json!({"kind":"map","name":name,"state":state(),
        "over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[],
        "body":{"kind":"fail","name":format!("{name}_invalid"),"reason":"invalid_retry_state"}})
}

#[tokio::test]
async fn production_retry_guard_maps_do_not_hide_rejected_review_completions() {
    let mut body = review_body();
    let reviews = body["children"]
        .as_array_mut()
        .assert_value()
        .pop()
        .assert_value();
    body["children"].as_array_mut().assert_value().push(error_exit(
        "worker_result",
        "worker",
        sequence("review_result", vec![reviews, json!({
            "kind":"choice","name":"retry_result","state":state(),"promotedStatePaths":[],
            "branches":[{"when":control("acceptance",ControlSource::Signal,&["rejected"]),
                "node":retry_guard("review_retry")}],
            "otherwise":retry_guard("delivery_retry")
        })]),
    ));
    let mut case = Case::new(looped(
        "change_loop",
        body,
        Some(until(&["acceptance", "code"])),
        10,
    ))
    .await;
    let mut rejected = BTreeSet::new();
    for _ in 0..4 {
        let references = case.round(
            &["worker"],
            &[("acceptance", "rejected"), ("code", "accepted")],
            &[],
        );
        rejected.insert(references[0].clone());
    }
    let evidence = case.impact();
    assert_eq!(
        evidence.episodes["acceptance"].completion_references,
        rejected
    );
    assert!(!evidence.episodes["worker"].allows(&rejected.into_iter().collect::<Vec<_>>()));
}

#[tokio::test]
async fn nested_map_and_loop_completions_remain_available_without_inferred_worker_blame() {
    let mut case = Case::new(mapped(looped("outer", review_loop(), None, 2))).await;
    let mut expected = BTreeSet::new();
    for scope in [&[0][..], &[1][..]] {
        case.visit("worker", verified(), scope);
        let check = case.visit("acceptance", reviewed("rejected"), scope);
        expected.insert(reference(&case, check));
    }
    let evidence = case.impact();
    assert_eq!(
        evidence.episodes["acceptance"].completion_references,
        expected
    );
    assert!(!evidence.episodes["worker"].allows(&expected.into_iter().collect::<Vec<_>>()));
}

#[tokio::test]
async fn retry_seed_and_incomplete_siblings_do_not_hide_a_real_completion() {
    let mut case = Case::new(review_loop()).await;
    let worker = case.visit("worker", verified(), &[]);
    let check = case.visit("acceptance", reviewed("rejected"), &[]);
    let sibling = case.visit("code", reviewed("accepted"), &[]);
    case.snapshot
        .executions
        .get_mut(&check)
        .assert_value()
        .attempt = PositiveInteger::new(2).assert_value();
    let seed = &case.snapshot.executions[&worker];
    case.snapshot.execution_seed.push(DurableExecution {
        dispatch_position: HistoryPosition::new(1).assert_value(),
        node_instance: seed.reference.node_instance,
        execution: seed.reference.execution,
        occurrence: seed.occurrence.clone(),
        attempt: seed.attempt,
        input: seed.input.clone(),
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(2).assert_value(),
            outcome: verified(),
        },
    });
    for sibling_state in [
        NodeState::Active,
        NodeState::Voided {
            at: cursor_for(100),
            reason: ExecutionVoidReason::ParallelJoin,
        },
    ] {
        case.snapshot
            .executions
            .get_mut(&sibling)
            .assert_value()
            .state = sibling_state;
        let evidence = case.impact();
        assert!(evidence.episodes["acceptance"].allows(&[reference(&case, check)]));
        assert!(!evidence.episodes.contains_key("code"));
    }
}

#[tokio::test]
async fn one_short_tool_observation_is_evidence_for_either_discovery_specialist() {
    let mut case = Case::new(review_loop()).await;
    let worker = case.visit("worker", verified(), &[]);
    case.snapshot
        .executions
        .get_mut(&worker)
        .assert_value()
        .state = NodeState::Active;
    let evidence = with_tools(&case, vec![observation(1, "worker", worker)]);
    let episode = &evidence.episodes["worker"];
    assert!(episode.completion_references.is_empty());
    assert!(episode.allows(&["hook:1".to_owned()]));
    assert_eq!(evidence.tools.len(), 1);
}

#[tokio::test]
async fn evidence_eligibility_requires_a_reference_owned_by_the_named_main_node() {
    let mut case = Case::new(review_loop()).await;
    let worker = case.visit("worker", verified(), &[]);
    let reviewer = case.visit("acceptance", reviewed("rejected"), &[]);
    let own = reference(&case, worker);
    let other = reference(&case, reviewer);
    let evidence = case.impact();
    let episode = &evidence.episodes["worker"];
    for refs in [
        vec![],
        vec![other.clone()],
        vec!["v2:999".to_owned()],
        vec!["node:worker".to_owned()],
    ] {
        assert!(!episode.allows(&refs));
    }
    assert!(episode.allows(&[own, other]));
}

#[tokio::test]
async fn malformed_foreign_and_auxiliary_tool_observations_are_excluded() {
    let mut case = Case::new(review_loop()).await;
    let worker = case.visit("worker", verified(), &[]);
    let mut auxiliary = case.snapshot.executions[&worker].clone();
    let auxiliary_id = ExecutionId::new(999).assert_value();
    auxiliary.reference.execution = auxiliary_id;
    auxiliary.reference.node = "active_memory_repository".parse().assert_value();
    auxiliary.occurrence.node = auxiliary.reference.node.clone();
    case.snapshot.executions.insert(auxiliary_id, auxiliary);
    let mut zero_time = observation(3, "worker", worker);
    zero_time.timestamp_ms = 0;
    let mut long_name = observation(4, "worker", worker);
    long_name.tool_name = "x".repeat(129);
    let mut many_paths = observation(5, "worker", worker);
    many_paths.paths = vec!["src/provider.rs".to_owned(); 5];
    let evidence = with_tools(
        &case,
        vec![
            observation(1, "worker", worker),
            observation(2, "acceptance", worker),
            observation(0, "worker", worker),
            zero_time,
            long_name,
            many_paths,
            observation(6, "active_memory_repository", auxiliary_id),
            observation(7, "worker", ExecutionId::new(998).assert_value()),
        ],
    );
    assert_eq!(evidence.tools.len(), 1);
    assert_eq!(
        evidence.episodes["worker"].tool_references,
        BTreeSet::from(["hook:1".to_owned()])
    );
    assert!(!evidence.episodes.contains_key("active_memory_repository"));
    assert!(!evidence.references().contains("hook:6"));
}
