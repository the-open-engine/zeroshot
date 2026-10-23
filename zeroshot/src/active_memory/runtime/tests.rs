use super::*;
use crate::active_memory::{LessonDraft, LessonEvidence, LessonScope};
use serde_json::json;

fn source(budget: usize) -> RunMemorySource {
    let lesson = LessonDraft {
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
    .into_lesson(1);
    RunMemorySource {
        index: MemoryIndex::new(MemorySnapshot {
            generation: 1,
            lessons: vec![lesson],
        })
        .unwrap(),
        workspace: PathBuf::from("/workspace"),
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
        tool_input: json!({"cmd":"cat guard.rs", "workdir":"/workspace/src"}),
        cwd: PathBuf::from("/workspace"),
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
        Value::String(json!({"cmd":"cat guard.rs", "workdir":"/workspace/src"}).to_string());
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
