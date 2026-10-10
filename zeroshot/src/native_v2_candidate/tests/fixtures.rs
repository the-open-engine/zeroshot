use openengine_cluster_protocol::GraphSpec;

use super::*;
use crate::native_v2_candidate::test_support::git_delivery_node;

pub(super) const CODEX_OPENAI: RuntimeLane = RuntimeLane::Codex {
    provider: CodexProvider::OpenAi,
};
pub(super) const CLAUDE_ANTHROPIC: RuntimeLane = RuntimeLane::Claude {
    provider: ClaudeProvider::Anthropic,
};
pub(super) const COPILOT_GITHUB: RuntimeLane = RuntimeLane::Copilot {
    provider: CopilotProvider::Github,
};

#[derive(Clone, Copy)]
pub(super) enum RuntimePlanKind {
    Codex,
    Claude,
    ClaudeOverride,
    ClaudeReviewer,
    CopilotReviewer,
}

impl RuntimePlanKind {
    fn reviewer_lane(self) -> Option<RuntimeLane> {
        match self {
            Self::ClaudeReviewer => Some(CLAUDE_ANTHROPIC),
            Self::CopilotReviewer => Some(COPILOT_GITHUB),
            Self::Codex | Self::Claude | Self::ClaudeOverride => None,
        }
    }
}

pub(super) fn agent_binding(lane: Option<RuntimeLane>, model: &str) -> NodeRuntimeBinding {
    NodeRuntimeBinding::Agent {
        lane,
        model: worker_catalog::ModelId::new(model).assert_value_with("model"),
        effort: Some(ReasoningEffort::Max),
        session_scope: SessionScope::Execution,
        connections: DeclaredConnections::empty(),
    }
}

pub(super) fn delivery_binding() -> NodeRuntimeBinding {
    NodeRuntimeBinding::GitDelivery {
        connections: DeclaredConnections::single(
            "github",
            DeclaredEnvironment::new([
                EnvironmentVariableName::new(GITHUB_TOKEN_ENV).assert_value_with("token name")
            ])
            .assert_value_with("delivery environment"),
        )
        .assert_value_with("delivery connection"),
        pull_request_feedback: Default::default(),
    }
}

pub(super) fn runtime(kind: RuntimePlanKind) -> RuntimePlan {
    let worker = match kind {
        RuntimePlanKind::Codex
        | RuntimePlanKind::ClaudeReviewer
        | RuntimePlanKind::CopilotReviewer => agent_binding(None, "gpt-5.6-sol"),
        RuntimePlanKind::Claude => agent_binding(None, "claude-sonnet-5"),
        RuntimePlanKind::ClaudeOverride => agent_binding(Some(CLAUDE_ANTHROPIC), "claude-sonnet-5"),
    };
    let mut nodes = BTreeMap::from([
        (
            NodeName::new("worker").assert_value_with("worker name"),
            worker,
        ),
        (
            NodeName::new("deliver").assert_value_with("delivery name"),
            delivery_binding(),
        ),
    ]);
    if let Some(lane) = kind.reviewer_lane() {
        nodes.insert(
            NodeName::new("reviewer").assert_value_with("reviewer name"),
            agent_binding(Some(lane), "claude-sonnet-5"),
        );
    }
    match kind {
        RuntimePlanKind::Codex
        | RuntimePlanKind::ClaudeOverride
        | RuntimePlanKind::ClaudeReviewer
        | RuntimePlanKind::CopilotReviewer => RuntimePlan::Codex {
            provider: CodexProvider::OpenAi,
            size: RunSize::Medium,
            nodes,
        },
        RuntimePlanKind::Claude => RuntimePlan::Claude {
            provider: ClaudeProvider::Anthropic,
            size: RunSize::Medium,
            nodes,
        },
    }
}

pub(super) fn shipping_graph(kind: RuntimePlanKind) -> GraphSpec {
    let receipt_type = serde_json::to_value(
        crate::native_v2_delivery::delivery_result_schema(
            crate::native_v2_delivery::DeliveryMode::Merge,
        )
        .assert_value_with("delivery result schema"),
    )
    .assert_value_with("delivery receipt type");
    let fields = receipt_type
        .pointer("/fields")
        .and_then(Value::as_object)
        .map(|fields| fields.keys().cloned().collect::<Vec<_>>())
        .assert_value_with("delivery receipt fields");
    let mut delivery = git_delivery_node();
    *delivery.assert_key_mut("writeBindings") = Value::Array(
        fields
            .iter()
            .map(|field| {
                json!({
                    "value":{"node":"deliver","channel":"out","path":[field]},
                    "target":["delivery",field]
                })
            })
            .collect(),
    );
    let state_type = json!({
        "kind":"record",
        "fields":{"delivery":{"type":receipt_type.clone(),"required":false}}
    });
    let terminal_type = json!({
        "kind":"record",
        "fields":{"delivery":{"type":receipt_type,"required":true}}
    });
    let terminal_bindings = fields
        .iter()
        .map(|field| {
            json!({
                "target":["delivery",field],
                "value":{"source":"state","path":["delivery",field]}
            })
        })
        .collect::<Vec<_>>();
    let mut children = vec![json!({
        "kind":"step","name":"worker","worker":"agent.worker@1",
        "instructions":"Exercise the candidate worker.",
        "input":{"kind":"null"},"output":{"kind":"null"},
        "inputBindings":[],"writeBindings":[],"timeoutMs":10000,"attempts":1
    })];
    if kind.reviewer_lane().is_some() {
        children.push(json!({
            "kind":"verifier","name":"reviewer","worker":"agent.reviewer@1",
            "instructions":"Review the candidate worker.",
            "input":{"kind":"null"},"output":{"kind":"null"},
            "inputBindings":[],"writeBindings":[],"timeoutMs":10000,"attempts":1,
            "signals":{},"diagnostic":{"kind":"null"}
        }));
    }
    children.push(delivery);
    children.push(json!({
        "kind":"choice","name":"delivery_result","state":state_type,
        "branches":[{
            "when":{
                "kind":"in",
                "value":{"name":"deliver","source":"signal","field":"delivery"},
                "labels":["merged"]
            },
            "node":{
                "kind":"succeed","name":"done","output":terminal_type,
                "bindings":terminal_bindings
            }
        }],
        "otherwise":{"kind":"fail","name":"delivery_failed","reason":"delivery_failed"},
        "promotedStatePaths":[]
    }));
    serde_json::from_value(json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":state_type,
        "policy":{"policy":"policy.native-v2@1","default":"deny"},
        "root":{
            "kind":"seq",
            "name":"root",
            "state":state_type,
            "children":children,
            "promotedStatePaths":[]
        }
    }))
    .assert_value_with("shipping graph")
}

pub(super) async fn admitted(kind: RuntimePlanKind) -> AdmittedRun {
    NativeV2Admission
        .admit(RunSubmission {
            environment: None,
            title: RunTitle::new("Candidate config").assert_value_with("title"),
            graph: shipping_graph(kind),
            initial_input: json!({}),
            runtime: runtime(kind),
            source: ResolvedSource {
                repository: SourceRepositoryId::new("acme/project").assert_value_with("repository"),
                branch: SourceBranchId::new("main").assert_value_with("target branch"),
                revision: SourceRevisionId::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                    .assert_value_with("base revision"),
            },
            submission_key: IdempotencyKey::new("candidate-config").assert_value_with("key"),
        })
        .await
        .assert_value_with("admitted")
}

pub(super) fn candidate_config(
    kind: RuntimePlanKind,
    repository: &TempRepository,
    github: Arc<ScriptedGitHub>,
) -> NativeV2CandidateConfig {
    NativeV2CandidateConfig {
        lanes: runtime(kind)
            .lanes()
            .into_iter()
            .map(|lane| harness_config(lane, repository))
            .collect(),
        delivery: NativeV2DeliveryConfig {
            delivery_run_id: RunId::new("candidate-fixture"),
            adopt_existing_delivery: false,
            git_identity: None,
            workspace: repository.workspace.clone(),
            git_program: PathBuf::from("/usr/bin/git"),
            target: DeliveryTarget::new("acme/project", "main", repository.base.clone())
                .assert_value_with("target"),
            poll: DeliveryPollPolicy::new(2, Duration::ZERO).assert_value_with("poll"),
        },
        github,
    }
}

pub(super) fn harness_config(
    lane: RuntimeLane,
    repository: &TempRepository,
) -> NativeV2HarnessConfig {
    let pool = HostedProcessPool::new(10_002, 10_002, 20_000).assert_value_with("pool");
    match lane {
        RuntimeLane::Copilot { .. } => NativeV2HarnessConfig::Copilot(CopilotConfig {
            executable: PathBuf::from("/usr/bin/false"),
            workspace: repository.workspace.clone(),
            runtime_home: repository.root.child("copilot-runtime"),
            local_user: None,
            base_environment: BTreeMap::new(),
            local_command_environment: BTreeMap::new(),
            search_path: "/usr/bin:/bin".to_owned(),
            process_pool: pool,
        }),
        RuntimeLane::Codex { provider } => NativeV2HarnessConfig::Codex(NativeV2CodexConfig {
            base_environment: Default::default(),
            provider,
            executable: PathBuf::from("/usr/bin/false"),
            workspace: repository.workspace.clone(),
            runtime_home: repository.root.child("codex-home"),
            local_user: None,
            native_environment: Default::default(),
            search_path: "/usr/bin:/bin".to_owned(),
            process_pool: pool,
        }),
        RuntimeLane::Claude { provider } => NativeV2HarnessConfig::Claude(ClaudeAdapterConfig {
            provider,
            executable: "/usr/bin/false".to_owned(),
            prefix_arguments: Vec::new(),
            workspace: repository.workspace.clone(),
            runtime_home: repository.root.child("claude-runtime"),
            local_user_home: None,
            native_environment: Default::default(),
            base_environment: ClaudeProcessEnvironment::new(BTreeMap::from([
                (
                    "HOME".to_owned(),
                    repository.root.path().to_string_lossy().into_owned(),
                ),
                ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ]))
            .assert_value_with("Claude environment"),
            process_pool: pool,
        }),
    }
}

pub(super) async fn wait_for_terminal(
    controller: &NativeV2CloudController,
    run_id: &RunId,
) -> TerminalResult {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = ClusterBackend::run_status(
                controller,
                &ConnectionContext::default(),
                RunStatusParams {
                    run_id: run_id.clone(),
                },
            )
            .await
            .assert_value_with("OECP status");
            if let RunStatus::Finished {
                terminal_result, ..
            } = status.status
            {
                return terminal_result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .assert_value_with("candidate became terminal")
}

use openengine_cluster_testkit::assertions::{AssertValue};
