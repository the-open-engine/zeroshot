use super::*;
use crate::active_memory::{LessonDraft, LessonEvidence, LessonScope};
use serde_json::json;

fn repository_lesson() -> crate::active_memory::Lesson {
    LessonDraft {
        id: "runtime-recall".into(),
        scope: LessonScope::Repository {
            repository: "repo:fixture".into(),
            files: vec!["src/guard.rs".into()],
            folders: vec![],
            identifiers: vec![],
            keyword_groups: vec![],
        },
        title: "Preserve sibling guards".into(),
        recommendation: "Check every credential selector when changing the guard matrix.".into(),
        evidence: LessonEvidence {
            run_id: "origin".into(),
            node_id: "worker".into(),
            references: vec!["execution:1".into()],
            costly_recurring_reason: "Two repair rounds missed sibling selectors.".into(),
        },
    }
    .into_lesson(1)
}

fn workspace() -> PathBuf {
    if cfg!(windows) {
        "C:/workspace".into()
    } else {
        "/workspace".into()
    }
}

fn source(budget: usize) -> RunMemorySource {
    RunMemorySource {
        index: MemoryIndex::new(MemorySnapshot {
            generation: 1,
            lessons: vec![repository_lesson()],
        })
        .unwrap(),
        workspace: workspace(),
        budget,
        repository_enabled: true,
        graph_enabled: false,
        nodes: BTreeSet::from(["worker".into()]),
        state: Mutex::default(),
    }
}

fn query(session: &str) -> HookQuery {
    HookQuery {
        node_id: "worker".into(),
        session_key: session.into(),
        execution_id: 1,
        native_session_id: None,
        tool_name: "exec_command".into(),
        tool_input: json!({"cmd":"cat guard.rs", "workdir":workspace().join("src")}),
        cwd: workspace(),
    }
}

#[test]
fn carrier_reuses_workdir_and_deduplicates_within_each_provider_session() {
    let source = source(2048);
    assert!(
        source
            .tool_context(&query("one"))
            .contains("Preserve sibling guards")
    );
    assert!(source.tool_context(&query("one")).is_empty());
    assert!(
        source
            .tool_context(&query("two"))
            .contains("Preserve sibling guards")
    );
    assert_eq!(source.manifest().unwrap().lessons.len(), 1);
}

#[test]
fn carrier_preserves_shell_cd_signals_and_decodes_native_json_arguments() {
    let source = source(2048);
    let mut command = query("one");
    command.tool_input = json!({"cmd":"cd src && cat guard.rs"});
    assert!(
        source
            .tool_context(&command)
            .contains("Preserve sibling guards")
    );
    command.session_key = "two".into();
    command.tool_input =
        Value::String(json!({"cmd":"cat guard.rs", "workdir":workspace().join("src")}).to_string());
    assert!(
        source
            .tool_context(&command)
            .contains("Preserve sibling guards")
    );
}

#[test]
fn maintenance_abstains_after_an_unavailable_carrier_and_global_agents_do_not_recall() {
    let source = source(2048);
    assert!(source.manifest().unwrap().lessons.is_empty());
    let mut global = query("global");
    global.node_id = "postmortem.repository".into();
    assert!(source.tool_context(&global).is_empty());
    assert!(source.manifest().unwrap().lessons.is_empty());
    source.unavailable("callback failed");
    assert!(source.manifest().is_none());
}

#[test]
fn insufficient_budget_never_marks_an_undelivered_lesson_supplied() {
    let source = source(200);
    let first = source.tool_context(&query("one"));
    assert!(first.len() <= 200);
    let second = source.tool_context(&query("one"));
    assert!(first.len() + second.len() <= 200);
    assert!(source.manifest().unwrap().lessons.is_empty());
}

#[test]
fn untrusted_forge_metadata_cannot_select_a_different_repository_scope() {
    let claimed: openengine_cluster_protocol::ResolvedGitRemote = serde_json::from_value(json!({
        "url":"https://github.com/public/actual.git", "forge":{"provider":"github","repository":"private/claimed"}
    })).unwrap();
    let mut honest = claimed.clone();
    honest.forge = None;
    assert_eq!(remote_identity(&claimed.url), remote_identity(&honest.url));
    assert!(!remote_identity(&claimed.url).contains("claimed"));
    honest.url =
        openengine_cluster_protocol::GitRemoteUrl::new("https://github.com/private/claimed.git")
            .unwrap();
    assert_ne!(remote_identity(&claimed.url), remote_identity(&honest.url));
}

#[test]
fn oversized_public_tool_inputs_are_bounded_on_utf8_boundaries() {
    let mut retrieval = RetrievalQuery::default();
    let mut fields = 0;
    collect_tool_input(
        &json!({"content":"é".repeat(MAX_QUERY_BYTES)}),
        &mut retrieval,
        &mut fields,
        0,
    );
    assert!(retrieval.text.len() <= MAX_QUERY_BYTES);
    assert!(retrieval.text.is_char_boundary(retrieval.text.len()));
}

#[test]
fn tool_receipts_exclude_arguments_global_agents_and_outside_paths() {
    let mut source = source(2048);
    source.repository_enabled = false;
    let mut call = query("worker");
    call.tool_input = json!({"file_path":"src/guard.rs", "token":"private-fixture-value"});
    assert!(source.tool_context(&call).is_empty());
    call.tool_input = json!({"file_path":"../../outside"});
    source.tool_context(&call);
    call.node_id = "active_memory_repository".into();
    source.tool_context(&call);
    let receipts = source.tool_observations();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0].paths, ["src/guard.rs"]);
    assert!(receipts[1].paths.is_empty());
    assert!(
        !serde_json::to_string(&receipts)
            .unwrap()
            .contains("private-fixture-value")
    );
}

#[test]
fn bounded_tool_receipts_preserve_first_and_latest_activity() {
    let source = source(2048);
    for _ in 0..MAX_TOOL_OBSERVATIONS + 20 {
        source.tool_context(&query("one"));
    }
    let receipts = source.tool_observations();
    assert_eq!(receipts.len(), MAX_TOOL_OBSERVATIONS);
    assert_eq!(receipts[0].seq, 1);
    assert_eq!(
        receipts.last().unwrap().seq,
        (MAX_TOOL_OBSERVATIONS + 20) as u64
    );
}

fn invocation(
    scope: crate::execution::SessionScope,
    execution: u64,
) -> crate::native_v2_runner::DriverInvocation {
    use crate::native_v2_runner::{DriverInvocation, NodeResponseContract, NodeRole};
    let mut request =
        crate::native_v2_runner::test_support::request("memory-runtime", "worker", (1, execution));
    if let openengine_cluster_protocol::NodeRuntimeBinding::Agent { session_scope, .. } =
        &mut request.invocation.binding
    {
        *session_scope = scope;
    }
    DriverInvocation {
        host_context: None,
        provider_session_slot: request.invocation.reference.node_instance,
        node: request.invocation,
        role: NodeRole::Worker,
        response: NodeResponseContract::Worker {
            output: openengine_cluster_protocol::PayloadType::Null,
        },
        environment: request.environment,
        session: Arc::new(crate::native_v2_runner::test_support::CountingSession {
            closed: Arc::default(),
        }),
    }
}

fn graph_lesson() -> crate::active_memory::Lesson {
    let mut lesson = repository_lesson();
    lesson.id = "runtime-graph".into();
    lesson.scope = LessonScope::Graph {
        graph_hash: "a".repeat(64),
        node_id: "worker".into(),
        repository: None,
    };
    lesson.title = "Check declared outputs".into();
    lesson.recommendation = "Check every declared output against its schema before returning. Inspect required fields and nested values, and verify errors use the expected channel before completing the node.".into();
    lesson
}

#[test]
fn fresh_execution_sessions_recall_repository_cards_while_reused_sessions_deduplicate() {
    use crate::execution::SessionScope;
    for scope in [SessionScope::Execution, SessionScope::NodeInstance] {
        let source = Arc::new(source(2048));
        let hooks = ActiveMemoryHooks::new(source.clone()).unwrap();
        let tool = json!({"tool_name":"Read", "tool_input":{"file_path":"src/guard.rs"}});
        let first = invocation(scope, 2);
        assert!(
            hooks
                .copilot_context(&first, &tool, &source.workspace)
                .contains("Preserve sibling guards")
        );
        assert!(
            hooks
                .copilot_context(&first, &tool, &source.workspace)
                .is_empty()
        );
        let next = invocation(scope, 3);
        let recalled = hooks.copilot_context(&next, &tool, &source.workspace);
        assert_eq!(
            recalled.contains("Preserve sibling guards"),
            scope == SessionScope::Execution
        );
    }
}

#[test]
fn graph_guidance_and_tool_callbacks_share_one_budget_with_the_configured_session_lifetime() {
    use crate::execution::SessionScope;
    for scope in [SessionScope::Execution, SessionScope::NodeInstance] {
        let mut source = source(2048);
        source.graph_enabled = true;
        source.index = MemoryIndex::new(MemorySnapshot {
            generation: 1,
            lessons: vec![repository_lesson(), graph_lesson()],
        })
        .unwrap();
        source.budget = source
            .index
            .graph_guidance("worker", 2048, &BTreeSet::new())
            .bytes_used;
        let source = Arc::new(source);
        let hooks = ActiveMemoryHooks::new(source.clone()).unwrap();
        let first = invocation(scope, 2);
        let guided = hooks.invocation(first.clone());
        assert!(
            guided
                .node
                .instructions
                .unwrap()
                .as_str()
                .contains("Check declared outputs")
        );
        let tool = json!({"tool_name":"Read", "tool_input":{"file_path":"src/guard.rs"}});
        assert!(
            hooks
                .copilot_context(&first, &tool, &source.workspace)
                .is_empty()
        );
        let repeated = hooks.invocation(first);
        assert!(
            !repeated
                .node
                .instructions
                .unwrap()
                .as_str()
                .contains("Check declared outputs")
        );
        let next = invocation(scope, 3);
        let recalled = hooks.copilot_context(&next, &tool, &source.workspace);
        assert_eq!(
            recalled.contains("Preserve sibling guards"),
            scope == SessionScope::Execution
        );
    }
}

#[test]
fn fresh_execution_sessions_receive_graph_guidance_again() {
    use crate::execution::SessionScope;
    for scope in [SessionScope::Execution, SessionScope::NodeInstance] {
        let mut source = source(2048);
        source.graph_enabled = true;
        source.index = MemoryIndex::new(MemorySnapshot {
            generation: 1,
            lessons: vec![graph_lesson()],
        })
        .unwrap();
        let hooks = ActiveMemoryHooks::new(Arc::new(source)).unwrap();
        let first = hooks.invocation(invocation(scope, 2));
        assert!(
            first
                .node
                .instructions
                .unwrap()
                .as_str()
                .contains("Check declared outputs")
        );
        let next = hooks.invocation(invocation(scope, 3));
        assert_eq!(
            next.node
                .instructions
                .unwrap()
                .as_str()
                .contains("Check declared outputs"),
            scope == SessionScope::Execution
        );
    }
}

#[test]
fn preparation_requires_host_storage_and_rejects_invalid_or_checkout_owned_paths() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("checkout");
    std::fs::create_dir(&workspace).unwrap();
    let storage = directory.path().join("memory").join("lessons.sqlite3");
    let mut admitted = crate::native_v2_runner::test_support::admitted();
    assert!(
        PreparedActiveMemory::prepare(MemoryPreparation {
            admitted: &admitted,
            workspace: &workspace,
            storage: None
        })
        .unwrap()
        .is_none()
    );
    for budget in [0, crate::active_memory::MAX_INJECTION_BYTES as u32 + 1] {
        admitted.runtime.active_memory_mut().budget_bytes = budget;
        assert!(matches!(
            PreparedActiveMemory::prepare(MemoryPreparation {
                admitted: &admitted,
                workspace: &workspace,
                storage: Some(&storage)
            }),
            Err(ActiveMemoryError::Invalid(_))
        ));
    }
    admitted.runtime.active_memory_mut().budget_bytes = 2048;
    let unsafe_path = workspace.join("memory.sqlite3");
    assert!(matches!(
        PreparedActiveMemory::prepare(MemoryPreparation {
            admitted: &admitted,
            workspace: &workspace,
            storage: Some(&unsafe_path)
        }),
        Err(ActiveMemoryError::UnsafeStorage)
    ));
    admitted.runtime.active_memory_mut().repository = false;
    admitted.runtime.active_memory_mut().graph = false;
    assert!(
        PreparedActiveMemory::prepare(MemoryPreparation {
            admitted: &admitted,
            workspace: &workspace,
            storage: Some(&storage)
        })
        .unwrap()
        .is_none()
    );
    assert!(!storage.exists());
}

#[test]
fn prepared_memory_keeps_a_frozen_snapshot_after_another_run_updates_the_store() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("checkout");
    std::fs::create_dir(&workspace).unwrap();
    let storage = directory.path().join("memory").join("lessons.sqlite3");
    let mut admitted = crate::native_v2_runner::test_support::admitted();
    admitted.source = None;
    let mut draft = repository_lesson().draft();
    if let LessonScope::Repository { repository, .. } = &mut draft.scope {
        *repository = repository_key(&admitted, &workspace);
    }
    let mut store = ActiveMemoryStore::open(&storage).unwrap();
    store
        .apply_batch(&[crate::active_memory::LessonMutation::Upsert {
            expected_revision: None,
            draft: Box::new(draft.clone()),
        }])
        .unwrap();
    let first = PreparedActiveMemory::prepare(MemoryPreparation {
        admitted: &admitted,
        workspace: &workspace,
        storage: Some(&storage),
    })
    .unwrap()
    .unwrap();
    draft.recommendation =
        "Verify the updated credential guard matrix before accepting this change.".into();
    store
        .apply_batch(&[crate::active_memory::LessonMutation::Upsert {
            expected_revision: Some(1),
            draft: Box::new(draft),
        }])
        .unwrap();
    let next = PreparedActiveMemory::prepare(MemoryPreparation {
        admitted: &admitted,
        workspace: &workspace,
        storage: Some(&storage),
    })
    .unwrap()
    .unwrap();
    let call = json!({"tool_name":"Read", "tool_input":{"file_path":"src/guard.rs"}});
    let invoked = invocation(crate::execution::SessionScope::Execution, 2);
    let old_context = first.hooks.copilot_context(&invoked, &call, &workspace);
    let new_context = next.hooks.copilot_context(&invoked, &call, &workspace);
    assert!(old_context.contains("Check every credential selector"));
    assert!(!old_context.contains("updated credential guard"));
    assert!(new_context.contains("updated credential guard"));
    assert_eq!(first.snapshot.lessons[0].revision, 1);
    assert_eq!(next.snapshot.lessons[0].revision, 2);
}

struct NoModelStartup;

#[test]
fn session_capacity_and_poisoned_state_abstain_without_claiming_complete_delivery() {
    let source = source(2048);
    {
        let mut state = source.state.lock().unwrap();
        state.sessions = (0..MAX_SESSIONS)
            .map(|index| (format!("used-{index}"), SessionExposure::default()))
            .collect();
    }
    assert!(source.tool_context(&query("new-session")).is_empty());
    assert!(source.manifest().is_none());
    assert!(
        source
            .tool_context(&query("used-0"))
            .contains("Preserve sibling guards")
    );
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _locked = source.state.lock().unwrap();
        panic!("simulate a failed callback while its state is locked");
    }));
    assert!(source.tool_context(&query("used-1")).is_empty());
    assert!(source.manifest().is_none());
}

#[test]
fn tool_input_depth_and_field_limits_preserve_bounded_callback_work() {
    let mut nested = json!({"file_path":"src/guard.rs"});
    for _ in 0..10 {
        nested = json!({"nested":nested});
    }
    let mut query = RetrievalQuery::default();
    let mut fields = 0;
    collect_tool_input(&nested, &mut query, &mut fields, 0);
    assert!(query.paths.is_empty());
    let mut values = vec![Value::Null; MAX_QUERY_FIELDS];
    values.push(json!({"file_path":"src/guard.rs"}));
    let mut fields = 0;
    collect_tool_input(&json!(values), &mut query, &mut fields, 0);
    assert_eq!(fields, MAX_QUERY_FIELDS);
    assert!(query.paths.is_empty());
    let mut query = RetrievalQuery {
        workspace: workspace(),
        ..Default::default()
    };
    let mut fields = 0;
    collect_tool_input(
        &json!({"cwd":"subdir", "files":[{"file":"one.rs"},{"filename":"two.rs"}], "flag":true, "attempt":3}),
        &mut query,
        &mut fields,
        0,
    );
    assert_eq!(query.tool_cwd.unwrap(), workspace().join("subdir"));
    assert_eq!(query.paths, ["one.rs", "two.rs"]);
}
#[async_trait]
impl crate::native_v2_runner::NodeDriver for NoModelStartup {
    async fn run(
        &self,
        _: crate::native_v2_runner::DriverInvocation,
        _: crate::native_v2_runner::DriverControl,
    ) -> Result<openengine_cluster_protocol::WorkerOutcome, NodeRunnerError> {
        panic!("host setup and cleanup must not start a model")
    }
}
#[async_trait]
impl crate::native_v2_runner::SessionFactory for NoModelStartup {
    async fn open(
        &self,
        _: &crate::native_v2_contract::NodeInvocation,
        _: &crate::native_v2_runner::ResolvedEnvironment,
    ) -> Result<Arc<dyn crate::native_v2_runner::NodeSession>, NodeRunnerError> {
        panic!("host setup and cleanup must not open a provider session")
    }
}

#[tokio::test]
async fn wrappers_preserve_runner_admission_and_postmortem_opt_out_without_model_startup() {
    for enabled in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("checkout");
        std::fs::create_dir(&workspace).unwrap();
        let storage = directory.path().join("memory").join("lessons.sqlite3");
        let mut admitted = crate::native_v2_runner::test_support::admitted();
        admitted.runtime.global_agents_mut().postmortem.enabled = enabled;
        let prepared = PreparedActiveMemory::prepare(MemoryPreparation {
            admitted: &admitted,
            workspace: &workspace,
            storage: Some(&storage),
        })
        .unwrap()
        .unwrap();
        let backend = Arc::new(NoModelStartup);
        let inner = NativeNodeRunner::new(&admitted, backend.clone(), backend).unwrap();
        let wrapped = prepared.wrap(&admitted, inner);
        assert_eq!(wrapped.postmortem().is_some(), enabled);
        let mut request =
            crate::native_v2_runner::test_support::request("memory-wrapper", "worker", (1, 2));
        request.invocation.reference.node =
            openengine_cluster_protocol::NodeName::new("not_admitted").unwrap();
        assert!(matches!(
            wrapped.start(request.clone()).await,
            Err(NodeRunnerError::InvalidRole)
        ));
        let unavailable = unavailable_memory(wrapped);
        let postmortem = unavailable.postmortem().unwrap();
        let (_, cancellation) = tokio::sync::watch::channel(false);
        let run_id = request.invocation.reference.run_id;
        let result = postmortem
            .run(
                PostmortemRequest {
                    run_id: run_id.clone(),
                    snapshot: crate::v2_run_ledger::RunSnapshot::admitted(
                        run_id.clone(),
                        &admitted,
                    ),
                    admitted: admitted.clone(),
                    primary_terminal: openengine_cluster_protocol::TerminalResult::Succeeded {
                        output: Value::Null,
                    },
                    environment: Arc::new(
                        crate::native_v2_supervisor::RunEnvironment::exact_for_admitted(
                            &admitted,
                            BTreeMap::new(),
                        )
                        .unwrap(),
                    ),
                    ledger: Arc::new(crate::v2_run_ledger::fake::FakeRunLedger::new()),
                },
                cancellation,
            )
            .await;
        assert!(matches!(result, Err(PostmortemError::Unavailable(_))));
        unavailable.close_run(&run_id).await;
    }
}
