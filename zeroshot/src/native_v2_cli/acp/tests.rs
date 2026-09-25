use std::collections::BTreeMap;

use acp::Agent as _;
use super::*;
use openengine_cluster_protocol::{
    ClaudeProvider, CodexProvider, CopilotProvider, DeclaredConnections, ModelId, NodeName,
    RecordField, RunSize,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};

#[test]
fn prompt_requires_exactly_one_nonempty_text_block() {
    assert_eq!(task_from_prompt(vec!["work".into()]).unwrap(), "work");
    assert!(task_from_prompt(Vec::new()).is_err());
    assert!(task_from_prompt(vec!["".into()]).is_err());
    assert!(task_from_prompt(vec!["one".into(), "two".into()]).is_err());
}

#[test]
fn terminal_metadata_preserves_raw_output() {
    let run_id = RunId::new("run");
    let output = json!({ "response": "done" });
    let turn = TurnResult::new(
        run_id.clone(),
        TerminalResult::Succeeded {
            output: output.clone(),
        },
        false,
    )
    .unwrap();
    assert_eq!(turn.message.as_deref(), Some("done"));
    assert_eq!(
        turn.meta(),
        Map::from_iter([(
            "zeroshot".to_owned(),
            json!({ "runId": run_id.as_str(), "rawOutput": output })
        )])
    );
}

#[test]
fn cancellation_while_idle_does_not_poison_the_next_turn() {
    let mut state = SessionState::default();
    assert!(state.cancel_target().is_none());

    let active = state.start_turn().unwrap();
    assert!(!active.cancelled.load(Ordering::Acquire));
}

#[test]
fn close_and_workspace_loss_fence_later_turns() {
    let mut closed = SessionState::default();
    let active = closed.start_turn().unwrap();
    let close_target = closed.close_target().unwrap().unwrap();
    assert!(Arc::ptr_eq(&active, &close_target));
    assert!(closed.start_turn().is_err());

    let mut lost = SessionState::default();
    let active = lost.start_turn().unwrap();
    let loss_target = lost.loss_target().unwrap();
    assert!(Arc::ptr_eq(&active, &loss_target));
    assert!(lost.start_turn().is_err());
}

#[test]
fn acp_materializes_non_native_local_provider_access_before_validation() {
    let mut runtime: RuntimePlan = serde_json::from_value(json!({
        "harness":"codex",
        "provider":"openrouter",
        "size":"medium",
        "nodes":{
            "work":{
                "kind":"agent",
                "model":"provider-owned-model",
                "sessionScope":"node_instance"
            }
        }
    }))
    .assert_value();

    assert!(runtime.connection_requirements().is_empty());
    materialize_acp_provider_access(&mut runtime).assert_value();
    assert_eq!(
        runtime
            .connection_requirements()
            .values()
            .flat_map(|names| names.iter())
            .map(|name| name.as_str())
            .collect::<Vec<_>>(),
        vec!["OPENROUTER_API_KEY"]
    );
}

#[tokio::test]
async fn agent_surface_advertises_its_contract_and_rejects_unsupported_session_inputs() {
    let core = Arc::new(AcpCore::new(
        acp_profile(runtime("codex", "node_instance", json!({}))),
        PathBuf::from("unused-for-rejected-requests"),
    ));
    let (updates, _receiver) = mpsc::unbounded_channel();
    let agent = AcpAgent { core, updates };

    let initialized = agent
        .initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
        .await
        .assert_value();
    assert_eq!(initialized.protocol_version, acp::ProtocolVersion::V1);
    let info = initialized.agent_info.assert_value();
    assert_eq!(info.name, "zeroshot");
    assert_eq!(info.title.as_deref(), Some("Zeroshot"));
    agent
        .authenticate(acp::AuthenticateRequest::new("unused"))
        .await
        .assert_value();

    let error = agent
        .new_session(acp::NewSessionRequest::new("/unused").mcp_servers(vec![
            acp::McpServer::Stdio(acp::McpServerStdio::new("unsupported", "false")),
        ]))
        .await
        .assert_error();
    assert_eq!(error.code, acp::ErrorCode::InvalidParams);
    assert!(error.message.contains("MCP servers"));

    let missing = acp::SessionId::new("missing");
    for error in [
        agent
            .cancel(acp::CancelNotification::new(missing.clone()))
            .await
            .assert_error(),
        agent
            .close_session(acp::CloseSessionRequest::new(missing.clone()))
            .await
            .assert_error(),
        agent
            .prompt(acp::PromptRequest::new(missing, Vec::new()))
            .await
            .assert_error(),
    ] {
        assert_eq!(error.code, acp::ErrorCode::InvalidParams);
    }
}

fn acp_graph() -> openengine_cluster_protocol::GraphSpec {
    let field = || json!({"type":{"kind":"string"},"required":true});
    let response = || json!({"kind":"record","fields":{"response":field()}});
    let mut graph = serde_json::to_value(
        BuiltinGraphTemplate::SingleWorker
            .materialize(TemplateDelivery::None)
            .assert_value(),
    )
    .assert_value();
    for pointer in ["/root/state/fields", "/root/children/1/state/fields"] {
        graph
            .pointer_mut(pointer)
            .and_then(Value::as_object_mut)
            .expect("single-worker state fields")
            .insert("response".to_owned(), field());
    }
    *graph
        .pointer_mut("/root/children/0/output")
        .expect("single-worker output") = response();
    *graph
        .pointer_mut("/root/children/0/writeBindings")
        .expect("single-worker writes") = json!([{
        "target":["response"],
        "value":{"node":"worker","channel":"out","path":["response"]}
    }]);
    *graph
        .pointer_mut("/root/children/1/otherwise/output")
        .expect("single-worker success output") = response();
    *graph
        .pointer_mut("/root/children/1/otherwise/bindings")
        .expect("single-worker success bindings") = json!([{
        "target":["response"],
        "value":{"source":"state","path":["response"]}
    }]);
    serde_json::from_value(graph).assert_value()
}

fn acp_profile(runtime: RuntimePlan) -> RunProfile {
    RunProfile {
        id: "profile-acp".to_owned(),
        name: RunProfileName::new("acp").assert_value(),
        scope: RunProfileScope::User,
        graph: acp_graph(),
        runtime,
        is_default: false,
    }
}

fn runtime(harness: &str, scope: &str, connections: Value) -> RuntimePlan {
    let binding = NodeRuntimeBinding::Agent {
        model: ModelId::new("provider-model").assert_value(),
        effort: None,
        session_scope: match scope {
            "node_instance" => SessionScope::NodeInstance,
            _ => SessionScope::Execution,
        },
        connections: serde_json::from_value::<DeclaredConnections>(connections).assert_value(),
    };
    let nodes = BTreeMap::from([(NodeName::new("worker").assert_value(), binding)]);
    match harness {
        "claude" => RuntimePlan::Claude {
            environment: None,
            provider: ClaudeProvider::Anthropic,
            size: RunSize::Small,
            nodes,
        },
        "copilot" => RuntimePlan::Copilot {
            environment: None,
            provider: CopilotProvider::Github,
            size: RunSize::Small,
            nodes,
        },
        _ => RuntimePlan::Codex {
            environment: None,
            provider: CodexProvider::OpenAi,
            size: RunSize::Small,
            nodes,
        },
    }
}

#[tokio::test]
async fn wave7_cli_contract_acp_profile_validation_rejects_unsupported_shapes() {
    let valid = runtime("codex", "node_instance", json!({}));
    validate_profile(&acp_profile(valid.clone()))
        .await
        .unwrap_or_else(|error| panic!("valid ACP profile was rejected: {error}"));

    let mut graph = serde_json::to_value(acp_graph()).assert_value();
    graph["root"]["children"]
        .as_array_mut()
        .assert_value()
        .pop();
    let no_success = RunProfile {
        graph: serde_json::from_value(graph).assert_value(),
        ..acp_profile(valid.clone())
    };
    assert!(
        validate_profile(&no_success)
            .await
            .assert_error()
            .to_string()
            .contains("at least one success")
    );
    assert!(
        validate_profile(&acp_profile(runtime("copilot", "node_instance", json!({}))))
            .await
            .assert_error()
            .to_string()
            .contains("only Codex and Claude")
    );
    assert!(
        validate_profile(&acp_profile(runtime(
            "codex",
            "node_instance",
            json!({"provider":["TOKEN"]})
        )))
        .await
        .assert_error()
        .to_string()
        .contains("connections are not supported")
    );
    assert!(
        validate_profile(&acp_profile(runtime("claude", "execution", json!({}))))
            .await
            .assert_error()
            .to_string()
            .contains("node_instance session scope")
    );

    let map: GraphNode = serde_json::from_value(json!({
        "kind":"map",
        "name":"items",
        "state":{"kind":"record","fields":{
            "items":{"type":{"kind":"array","items":{"kind":"string"}},"required":true}
        }},
        "body":{
            "kind":"succeed",
            "name":"done",
            "output":{"kind":"null"},
            "bindings":[]
        },
        "over":{"source":"state","path":["items"]},
        "maxItems":1,
        "promotedStatePaths":[]
    }))
    .assert_value();
    assert!(
        validate_node(&map, &mut 0)
            .assert_error()
            .to_string()
            .contains("map nodes")
    );
}

#[test]
fn wave7_cli_contract_acp_turns_state_and_payloads_preserve_failure_semantics() {
    let run_id = RunId::new("run-contract");
    let cancelled = TurnResult::new(
        run_id.clone(),
        TerminalResult::Succeeded {
            output: json!({"response":"ignored"}),
        },
        true,
    )
    .assert_value();
    assert_eq!(cancelled.stop_reason, acp::StopReason::Cancelled);
    assert!(cancelled.message.is_none());

    let reason = openengine_cluster_protocol::EnumLabel::new("runtime_failed").assert_value();
    let failed = TurnResult::new(
        run_id,
        TerminalResult::Failed {
            reason: reason.clone(),
        },
        false,
    )
    .assert_value();
    assert_eq!(failed.raw_output, json!({"failed":"runtime_failed"}));
    assert_eq!(
        failed.message.as_deref(),
        Some("Zeroshot run failed: runtime_failed")
    );
    assert_eq!(
        terminal_value(TerminalResult::Failed { reason }),
        json!({"failed":"runtime_failed"})
    );
    assert!(
        TurnResult::new(
            RunId::new("run-malformed"),
            TerminalResult::Succeeded {
                output: Value::Null
            },
            false,
        )
        .is_err()
    );

    let valid = PayloadType::Record {
        fields: std::collections::BTreeMap::from([(
            FieldName::new("task").assert_value(),
            RecordField {
                value_type: PayloadType::String,
                required: true,
            },
        )]),
    };
    assert!(validate_single_string_record(&valid, "task").is_ok());
    assert!(validate_single_string_record(&PayloadType::Null, "task").is_err());
    assert!(validate_single_string_record(&valid, "missing").is_err());
    assert!(
        validate_task_type(&PayloadType::Null)
            .assert_error()
            .to_string()
            .contains("required task string")
    );
    assert!(
        validate_response_type(&PayloadType::Null)
            .assert_error()
            .to_string()
            .contains("required response string")
    );

    let mut state = SessionState::default();
    let active = state.start_turn().assert_value();
    assert!(state.start_turn().is_err());
    let unrelated = Arc::new(ActiveTurn::new());
    state.finish_turn(&unrelated);
    assert!(state.active.is_some());
    state.finish_turn(&active);
    assert!(state.active.is_none());
    assert!(state.close_target().assert_value().is_none());
    assert!(state.close_target().is_err());
    assert!(state.cancel_target().is_none());
    assert!(state.loss_target().is_none());

    let active = ActiveTurn::new();
    assert!(active.leases_are_intact());
    assert!(!active.cancelled.load(Ordering::Acquire));
    assert!(!active.lost.load(Ordering::Acquire));

    let internal = AcpServeError::Transport("private detail".to_owned()).rpc_error();
    assert_eq!(internal.code, acp::ErrorCode::InternalError);
    assert!(!internal.message.contains("private detail"));
    assert!(matches!(
        prepare_session(Path::new("state"), PathBuf::from("relative")),
        Err(AcpServeError::Request("session cwd must be absolute"))
    ));
}

#[tokio::test]
async fn acp_rejects_environment_hooks_before_starting_a_session() {
    for definition in [
        json!({"setup":"echo root"}),
        json!({"startup":"echo checkout"}),
    ] {
        let runtime = runtime("codex", "node_instance", json!({}))
            .map_environment(|_| Some(serde_json::from_value(definition).assert_value()));
        assert!(matches!(
            validate_profile(&acp_profile(runtime)).await,
            Err(AcpServeError::Composition(
                LocalCompositionError::PreparationRequiresTarget
            ))
        ));
    }
    let runtime = runtime("codex", "node_instance", json!({})).map_environment(|_| {
        Some(serde_json::from_value(json!({"variables":{"CI":"true"}})).assert_value())
    });
    validate_profile(&acp_profile(runtime)).await.assert_value();
}
