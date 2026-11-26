use super::*;
use openengine_cluster_protocol::{RunWorkspace, WorkspacePlacement};

async fn workspace_case(in_git: bool) -> Case {
    let mut case = case().await;
    case.stored.admitted.source = None;
    case.stored.admitted.workspace = Some(RunWorkspace {
        placement: WorkspacePlacement::InPlace,
        local_directory: Some(case.workspace().to_str().assert_value().into()),
        seeds: Vec::new(),
        copy_root: false,
    });
    if !in_git {
        std::fs::remove_dir_all(case.workspace().join(".git")).assert_value();
    }
    write_history(&case.state, &case.workspace(), &case.stored);
    case
}

async fn initialize_graph_only(case: &Case) {
    let inventory = case.advance().await;
    assert!(inventory["excluded"].as_array().assert_value().is_empty());
    let runs = inventory["runs"].as_array().assert_value();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run["workspace"], json!(case.workspace()));
    assert!(run["repository"].is_null());
    assert!(run["remote_key"].is_null());
    assert!(
        run["repository_aliases"]
            .as_array()
            .assert_value()
            .is_empty()
    );
    assert_eq!(run["repository_enabled"], false);
    assert_eq!(run["graph_enabled"], true);
    let tasks = run["tasks"].as_array().assert_value();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["role"], "Graph");
    let task = &tasks[0];
    let payload = read(&case.session.join(task["path"].as_str().assert_value()));
    assert!(payload["context"]["repository"].is_null());
    assert!(payload["context"]["source_basis"].is_null());
    case.select(&inventory);

    // A graph discovery cannot acquire repository authority by asking for qualification.
    let mut finding = discovery(task);
    finding["lessons"][0]["references"] = json!(["v2:1", "v2:2"]);
    response(&case.session, task, finding.clone());
    assert!(initialize::review(&case.session).await.is_err());
    finding["lessons"][0]["repository_specific"] = json!(false);
    response(&case.session, task, finding);
    let review = case.advance().await;
    assert_eq!(review["staged_new_lessons"], 1);
    let tasks = review["maintenance_tasks"].as_array().assert_value();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["task"]["role"], "GraphMaintenance");
    let state = read(&case.session.join("review.json"));
    assert!(state["workspaces"].as_array().assert_value().is_empty());
    let lesson = &state["snapshot"]["lessons"][0];
    assert_eq!(lesson["scope"]["kind"], "graph");
    assert!(lesson["scope"]["repository"].is_null());

    case.maintenance(&review, "keep");
    let report = case.advance().await;
    assert_eq!(report["phase"], "complete");
    assert_eq!(report["published_mutations"], 1);
    let published = case
        .store()
        .record(lesson["id"].as_str().assert_value())
        .assert_value()
        .assert_value()
        .lesson
        .assert_value();
    assert!(matches!(
        published.scope,
        LessonScope::Graph {
            repository: None,
            ..
        }
    ));
}

#[tokio::test]
async fn workspace_history_inside_git_initializes_only_graph_memory() {
    initialize_graph_only(&workspace_case(true).await).await;
}

#[tokio::test]
async fn workspace_history_outside_git_initializes_only_graph_memory() {
    initialize_graph_only(&workspace_case(false).await).await;
}

#[tokio::test]
async fn disposed_managed_workspace_history_initializes_graph_memory() {
    let mut case = workspace_case(false).await;
    let workspace = case.stored.admitted.workspace.as_mut().assert_value();
    workspace.placement = WorkspacePlacement::Managed;
    workspace.local_directory = None;
    write_history(&case.state, &case.workspace(), &case.stored);
    std::fs::remove_dir_all(case.workspace()).assert_value();
    initialize_graph_only(&case).await;
}

#[tokio::test]
async fn workspace_history_with_graph_memory_opted_out_is_excluded() {
    let mut case = workspace_case(true).await;
    let config = case.stored.admitted.runtime.active_memory_mut();
    config.repository = true;
    config.graph = false;
    write_history(&case.state, &case.workspace(), &case.stored);
    let inventory = case.prepare().await;
    assert!(inventory["runs"].as_array().assert_value().is_empty());
    assert_eq!(inventory["excluded"].as_array().assert_value().len(), 1);
    let report = case.advance().await;
    assert_eq!(report["phase"], "complete");
    assert_eq!(report["published_mutations"], 0);
}
