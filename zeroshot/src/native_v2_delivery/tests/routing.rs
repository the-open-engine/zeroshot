use super::*;
use openengine_cluster_protocol::GraphSpec;
use openengine_cluster_testkit::assertions::JsonAt;

#[test]
fn hosted_delivery_polling_has_no_work_duration_limit() {
    assert!(DeliveryPollPolicy::default().has_next(usize::MAX));
    assert!(
        !DeliveryPollPolicy::new(3, Duration::ZERO)
            .assert_value()
            .has_next(3)
    );
}

struct RepairSession;

#[async_trait]
impl NodeSession for RepairSession {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn is_live(&self) -> bool {
        true
    }

    async fn close(&self) {}
}

struct DeliveryLoopLane {
    delivery: Arc<NativeV2DeliveryAdapter>,
    workspace: PathBuf,
    repairs: AtomicUsize,
}

#[async_trait]
impl SessionFactory for DeliveryLoopLane {
    async fn open(
        &self,
        invocation: &NodeInvocation,
        environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        match &invocation.binding {
            NodeRuntimeBinding::GitDelivery { .. } => {
                SessionFactory::open(self.delivery.as_ref(), invocation, environment).await
            }
            NodeRuntimeBinding::Agent { .. } => Ok(Arc::new(RepairSession)),
        }
    }
}

#[async_trait]
impl NodeDriver for DeliveryLoopLane {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        if matches!(
            &invocation.node.binding,
            NodeRuntimeBinding::GitDelivery { .. }
        ) {
            return NodeDriver::run(self.delivery.as_ref(), invocation, control).await;
        }
        assert_eq!(invocation.node.reference.node.as_str(), "repair");
        let repair = self.repairs.fetch_add(1, Ordering::SeqCst) + 1;
        let conflicted = self.workspace.join("result.txt");
        if fs::read_to_string(&conflicted).is_ok_and(|contents| contents.contains("<<<<<<<")) {
            fs::write(&conflicted, "resolved\n").map_err(|_| NodeRunnerError::Driver)?;
        }
        fs::write(
            self.workspace.join("repair.txt"),
            format!("repair {repair}\n"),
        )
        .map_err(|_| NodeRunnerError::Driver)?;
        Ok(WorkerOutcome::Verified {
            output: Value::Null,
            artifacts: Vec::new(),
        })
    }
}

async fn create_delivery_run(
    admitted: crate::native_v2_contract::AdmittedRun,
) -> (RunId, Arc<FakeRunLedger>, Arc<RunEnvironment>) {
    let run_id = RunId::new("delivery-supervisor-run");
    let ledger = Arc::new(FakeRunLedger::new());
    let environments = Arc::new(
        RunEnvironment::exact(
            &admitted.runtime,
            BTreeMap::from([(
                ConnectionKey::new("github").assert_value(),
                StaticConnectionValues::new(BTreeMap::from([(
                    EnvironmentVariableName::new(GITHUB_TOKEN_ENV).assert_value(),
                    "test-token".to_owned(),
                )]))
                .assert_value(),
            )]),
        )
        .assert_value_with("resolve delivery run environment"),
    );
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("delivery-supervisor").assert_value(),
            submission_digest: Sha256Digest::new("d".repeat(64)).assert_value(),
            admitted,
        })
        .await
        .assert_value_with("create delivery run");
    (run_id, ledger, environments)
}

async fn drive_repair_loop(
    repo: &TempRepo,
    authority: Arc<FakeGitHub>,
) -> (
    TerminalResult,
    Arc<DeliveryLoopLane>,
    RunId,
    Arc<FakeRunLedger>,
) {
    let admitted = admitted_routing_graph(&repo.base).await;
    let delivery = Arc::new(NativeV2DeliveryAdapter::new(
        NativeV2DeliveryConfig {
            workspace: repo.workspace.clone(),
            git_program: PathBuf::from("/usr/bin/git"),
            target: DeliveryTarget::new("acme/project", "main", repo.base.clone()).assert_value(),
            poll: DeliveryPollPolicy::new(3, Duration::ZERO).assert_value(),
        },
        authority,
    ));
    let lane = Arc::new(DeliveryLoopLane {
        delivery,
        workspace: repo.workspace.clone(),
        repairs: AtomicUsize::new(0),
    });
    let runner = Arc::new(
        NativeNodeRunner::new(&admitted, lane.clone(), lane.clone())
            .assert_value_with("delivery loop runner"),
    );
    let (run_id, ledger, environments) = create_delivery_run(admitted).await;
    let terminal = NativeV2Supervisor::new(run_id.clone(), ledger.clone(), runner, environments)
        .drive()
        .await
        .assert_value_with("drive delivery loop");
    (terminal, lane, run_id, ledger)
}

fn assert_successful_repair_loop(
    terminal: TerminalResult,
    repo: &TempRepo,
    lane: &DeliveryLoopLane,
    authority: &FakeGitHub,
) {
    let TerminalResult::Succeeded { output } = terminal else {
        panic!("delivery repair loop must succeed");
    };
    assert!(is_matching_success_receipt(
        &output,
        DeliveryMode::Merge,
        &target(repo)
    ));
    assert_eq!(lane.repairs.load(Ordering::SeqCst), 1);
    assert_eq!(authority.merge_requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn ci_failure_repairs_then_authoritatively_merges_in_one_supervised_run() {
    let repo = TempRepo::delivery();
    let authority = Arc::new(FakeGitHub::new(
        repo.remote.clone(),
        Script::CiFailsThenMerges,
    ));
    let (terminal, lane, run_id, ledger) = drive_repair_loop(&repo, authority.clone()).await;

    assert_successful_repair_loop(terminal, &repo, &lane, &authority);
    assert_eq!(authority.inspections.load(Ordering::SeqCst), 3);
    let stored = ledger.get(&run_id).await.assert_value().assert_value();
    let execution_nodes = stored
        .snapshot
        .executions
        .values()
        .map(|execution| execution.reference.node.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        execution_nodes
            .iter()
            .filter(|node| **node == "deliver")
            .count(),
        2
    );
    assert_eq!(
        execution_nodes
            .iter()
            .filter(|node| **node == "repair")
            .count(),
        1
    );
}

#[tokio::test]
async fn materialized_conflict_repairs_and_retries_the_same_run_branch() {
    let repo = TempRepo::delivery();
    let authority = Arc::new(FakeGitHub::new(
        repo.remote.clone(),
        Script::ConflictThenMerges,
    ));
    let (terminal, lane, _, _) = drive_repair_loop(&repo, authority.clone()).await;

    assert_successful_repair_loop(terminal, &repo, &lane, &authority);
    assert_eq!(
        authority.conflict_materializations.load(Ordering::SeqCst),
        1
    );
    let reviews = authority.review_requests();
    assert_eq!(reviews.len(), 2);
    assert_eq!(reviews[0].head_branch, reviews[1].head_branch);
    assert_ne!(reviews[0].head_revision, reviews[1].head_revision);
    assert_eq!(
        git_output(
            &repo.remote,
            &[
                "rev-parse",
                &format!("refs/heads/{}", reviews[1].head_branch)
            ]
        ),
        reviews[1].head_revision
    );
    assert_eq!(
        fs::read_to_string(repo.workspace.join("result.txt")).assert_value(),
        "resolved\n"
    );
}

pub(super) async fn assert_ci_failure_routes_an_authored_worker_loop(
    base_revision: &str,
    outcome: WorkerOutcome,
) {
    let admitted = admitted_routing_graph(base_revision).await;
    let verified = VerifiedGraph {
        compiled_ir: admitted.graph,
        diagnostics: Vec::new(),
    };
    let delivery = settled_execution((1, 1), "deliver", (0, 1), outcome);
    let after_failure = FullV1Reducer::native_v2(&verified)
        .reduce(ReductionInput {
            initial_input: &routing_initial_input(base_revision),
            executions: std::slice::from_ref(&delivery),
            next_node_instance: 2,
            next_execution: 2,
        })
        .assert_value();
    assert!(after_failure.decisions.iter().any(|decision| matches!(
        decision,
        Decision::Dispatch { occurrence, .. } if occurrence.node.as_str() == "repair"
    )));

    let repair = settled_execution(
        (2, 2),
        "repair",
        (2, 3),
        WorkerOutcome::Verified {
            output: Value::Null,
            artifacts: Vec::new(),
        },
    );
    let next_iteration = FullV1Reducer::native_v2(&verified)
        .reduce(ReductionInput {
            initial_input: &routing_initial_input(base_revision),
            executions: &[delivery, repair],
            next_node_instance: 3,
            next_execution: 3,
        })
        .assert_value();
    assert!(next_iteration.decisions.iter().any(|decision| matches!(
        decision,
        Decision::Dispatch { occurrence, node_instance, execution, .. }
            if occurrence.node.as_str() == "deliver"
                && node_instance.get() == 1
                && execution.get() == 3
    )));
}

async fn admitted_routing_graph(base_revision: &str) -> crate::native_v2_contract::AdmittedRun {
    let graph = routing_graph();
    let delivery = NodeRuntimeBinding::GitDelivery {
        connections: DeclaredConnections::single(
            "github",
            DeclaredEnvironment::new([
                EnvironmentVariableName::new(GITHUB_TOKEN_ENV).assert_value()
            ])
            .assert_value(),
        )
        .assert_value(),
    };
    let repair = NodeRuntimeBinding::Agent {
        model: crate::worker_catalog::ModelId::new("gpt-5.6").assert_value(),
        effort: Some(crate::worker_catalog::ReasoningEffort::Max),
        session_scope: crate::execution::SessionScope::Execution,
        connections: DeclaredConnections::empty(),
    };
    NativeV2Admission
        .admit(RunSubmission {
            title: RunTitle::new("Delivery routing test").assert_value(),
            graph,
            initial_input: routing_initial_input(base_revision),
            runtime: RuntimePlan::Codex {
                provider: crate::native_v2_contract::CodexProvider::OpenAi,
                size: RunSize::Medium,
                nodes: BTreeMap::from([
                    (NodeName::new("deliver").assert_value(), delivery),
                    (NodeName::new("repair").assert_value(), repair),
                ]),
            },
            source: ResolvedSource {
                repository: SourceRepositoryId::new("acme/project").assert_value(),
                branch: SourceBranchId::new("main").assert_value(),
                revision: SourceRevisionId::new(base_revision).assert_value(),
            },
            submission_key: IdempotencyKey::new("delivery-routing").assert_value(),
        })
        .await
        .assert_value()
}

fn routing_graph() -> GraphSpec {
    let state = serde_json::to_value(delivery_result_schema(DeliveryMode::Merge).assert_value())
        .assert_value();
    let fields = state
        .pointer("/fields")
        .and_then(Value::as_object)
        .map(|fields| fields.keys().cloned().collect::<Vec<_>>())
        .assert_value();
    let promoted = fields
        .iter()
        .map(|field| json!([field]))
        .collect::<Vec<_>>();
    let terminal_bindings = fields
        .iter()
        .map(|field| {
            json!({
                "target":[field],
                "value":{"source":"state","path":[field]}
            })
        })
        .collect::<Vec<_>>();
    let mut delivery = delivery_node(DeliveryMode::Merge);
    *delivery.assert_key_mut("writeBindings") = Value::Array(
        fields
            .iter()
            .map(|field| {
                let value = json!({"node":"deliver","channel":"out","path":[field]});
                json!({
                    "target":[field],
                    "value":value
                })
            })
            .collect(),
    );
    let repair = json!({
        "kind":"step","name":"repair","worker":"agent.repair@1",
        "instructions":"Repair the failed delivery.",
        "input":{"kind":"null"},"output":{"kind":"null"},
        "inputBindings":[],"writeBindings":[],"timeoutMs":1000,"attempts":1
    });
    let delivery_route = json!({
        "kind":"choice","name":"delivery_route","state":state.clone(),
        "branches":[{
            "when":{
                "kind":"in",
                "value":{"name":"deliver","source":"signal","field":"delivery"},
                "labels":["ci_failed","conflict"]
            },
            "node":repair
        }],
        "otherwise":{
            "kind":"succeed","name":"merged","output":state.clone(),
            "bindings":terminal_bindings.clone()
        },
        "promotedStatePaths":[]
    });
    let delivery_loop = json!({
        "kind":"loop","name":"delivery_loop","state":state.clone(),
        "body":{
            "kind":"seq","name":"delivery_attempt","state":state.clone(),
            "children":[delivery,delivery_route],
            "promotedStatePaths":promoted.clone()
        },
        "until":{
            "kind":"in",
            "value":{"name":"deliver","source":"signal","field":"delivery"},
            "labels":["merged"]
        },
        "maxIterations":3,"promotedStatePaths":promoted
    });
    let done = json!({
        "kind":"succeed","name":"done","output":state.clone(),
        "bindings":terminal_bindings
    });
    serde_json::from_value(json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":state.clone(),
        "policy":{"policy":"policy.native-v2@1","default":"deny"},
        "root":{
            "kind":"seq","name":"root","state":state,
            "children":[delivery_loop,done],
            "promotedStatePaths":[]
        }
    }))
    .assert_value()
}

fn routing_initial_input(base_revision: &str) -> Value {
    Value::Object(serde_json::Map::from_iter([
        ("version".to_owned(), json!("v2")),
        ("mode".to_owned(), json!("merge")),
        ("outcome".to_owned(), json!("conflict")),
        ("repository".to_owned(), json!("acme/project")),
        ("targetBranch".to_owned(), json!("main")),
        ("headRevision".to_owned(), json!(base_revision)),
        ("mergeRevision".to_owned(), json!("")),
        ("pullRequestId".to_owned(), json!("pending")),
    ]))
}

fn settled_execution(
    identity: (u64, u64),
    node: &str,
    positions: (u64, u64),
    outcome: WorkerOutcome,
) -> DurableExecution {
    let (execution, node_instance) = identity;
    let (dispatch_position, settle_position) = positions;
    DurableExecution {
        dispatch_position: HistoryPosition::new(dispatch_position).assert_value(),
        node_instance: native_v2_contract::NodeInstanceId::new(node_instance).assert_value(),
        execution: native_v2_contract::ExecutionId::new(execution).assert_value(),
        occurrence: StructuralOccurrence {
            node: NodeName::new(node).assert_value(),
            map_indices: Vec::new(),
        },
        attempt: PositiveInteger::new(1).assert_value(),
        input: Value::Null,
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(settle_position).assert_value(),
            outcome,
        },
    }
}

use openengine_cluster_testkit::assertions::{AssertValue};
