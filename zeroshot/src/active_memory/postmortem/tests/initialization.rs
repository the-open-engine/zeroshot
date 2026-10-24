//! Exercise the file bridge with isolated synthetic history; no model or real state is used.
use std::path::{Path, PathBuf};

use super::*;
use crate::active_memory::postmortem::initialize;
use crate::native_v2_contract::ExecutionId;
use crate::v2_run_ledger::{NodeState, StoredRun, cursor_for, cursor_sequence};
use crate::v2_run_ledger::sqlite::SqliteRunLedger;
use openengine_cluster_protocol::{WorkerErrorCode, WorkerFailureReason};

struct Case {
    fixture: Fixture,
    state: PathBuf,
    session: PathBuf,
    stored: StoredRun,
}

impl Case {
    fn workspace(&self) -> PathBuf {
        self.fixture._directory.path().join("workspace")
    }
    fn store(&self) -> ActiveMemoryStore {
        ActiveMemoryStore::open(&self.state.join("active-memory/lessons.sqlite3")).assert_value()
    }
    async fn prepare(&self) -> Value {
        initialize::prepare(self.state.clone(), &self.session, Default::default())
            .await
            .assert_value()
    }
    async fn stage(&self) -> Value {
        let inventory = self.prepare().await;
        self.select(&inventory);
        self.discoveries(&inventory, true);
        initialize::review(&self.session).await.assert_value()
    }
    async fn advance(&self) -> Value {
        initialize::init(self.state.clone(), &self.session, Default::default())
            .await
            .assert_value()
    }
    fn select(&self, inventory: &Value) {
        let runs = inventory["runs"].as_array().assert_value().iter().map(|run| json!({
            "run_id":run["run_id"],"include":true,"reason":"Software change with observed repeated failure evidence"})).collect::<Vec<_>>();
        write(&self.session.join("selection.json"), &json!({"runs":runs}));
    }
    fn discoveries(&self, inventory: &Value, findings: bool) {
        for run in inventory["runs"].as_array().assert_value() {
            for task in run["tasks"].as_array().assert_value() {
                let output = if findings {
                    discovery(task)
                } else {
                    json!({"lessons":[]})
                };
                response(&self.session, task, output);
            }
        }
    }
    fn maintenance(&self, review: &Value, action: &str) {
        self.reconcile(review, |lesson| maintenance(lesson, action));
    }
    fn reconcile(&self, review: &Value, decide: impl Fn(&Value) -> Value) {
        for entry in review["maintenance_tasks"].as_array().assert_value() {
            let task = &entry["task"];
            let payload = read(&self.session.join(task["path"].as_str().assert_value()));
            let decisions = payload["context"]["supplied_lessons"]
                .as_object()
                .assert_value()
                .values()
                .map(&decide)
                .collect::<Vec<_>>();
            response(&self.session, task, json!({"decisions":decisions}));
        }
    }
}

fn git(workspace: &Path, args: &[&str]) {
    assert!(
        std::process::Command::new("git")
            .current_dir(workspace)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .assert_value()
            .status
            .success()
    );
}

async fn case() -> Case {
    let fixture = fixture(Default::default(), None, true).await;
    fixture.drive().await;
    let workspace = fixture._directory.path().join("workspace");
    git(&workspace, &["init", "--quiet"]);
    git(&workspace, &["add", "src/provider.rs"]);
    let mut stored = fixture
        .ledger
        .get(&fixture.run_id)
        .await
        .assert_value()
        .assert_value();
    stored.snapshot.run_id = RunId::new(uuid::Uuid::now_v7().to_string());
    let template = stored
        .snapshot
        .executions
        .values()
        .next()
        .assert_value()
        .clone();
    stored.snapshot.executions.clear();
    for number in [1, 2] {
        let mut node = template.clone();
        node.reference.execution = ExecutionId::new(number).assert_value();
        node.state = NodeState::Completed {
            at: cursor_for(number),
            outcome: WorkerOutcome::Error {
                code: WorkerErrorCode::Crash,
                reason: WorkerFailureReason::DeclaredFailure,
            },
        };
        stored
            .snapshot
            .executions
            .insert(node.reference.execution, node);
    }
    let state = fixture._directory.path().join("state");
    let session = fixture._directory.path().join("session");
    write_history(&state, &workspace, &stored);
    Case {
        fixture,
        state,
        session,
        stored,
    }
}

fn write_history(state: &Path, workspace: &Path, stored: &StoredRun) {
    let directory = state.join("runs").join(stored.snapshot.run_id.as_str());
    crate::execution::platform::private_directory(&directory).assert_value();
    let path = directory.join("runs.sqlite3");
    drop(SqliteRunLedger::open(&path).assert_value());
    let connection = rusqlite::Connection::open(&path).assert_value();
    connection.execute("INSERT OR REPLACE INTO v2_runs(run_id,submission_key,submission_digest,cursor,stored_json) VALUES(?1,?2,?3,?4,?5)",
        rusqlite::params![stored.snapshot.run_id.as_str(),stored.submission_key.as_str(),stored.submission_digest.as_str(),
            cursor_sequence(&stored.snapshot.cursor).assert_value(),serde_json::to_string(stored).assert_value()]).assert_value();
    write(
        &directory.join("workspace-recovery.json"),
        &json!({"workspace":workspace}),
    );
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).assert_value()).assert_value()
}
fn write(path: &Path, value: &Value) {
    std::fs::write(path, serde_json::to_vec(value).assert_value()).assert_value();
}
fn response(directory: &Path, task: &Value, output: Value) {
    write(
        &directory.join(format!(
            "{}.response.json",
            task["path"].as_str().assert_value()
        )),
        &json!({"task_digest":task["digest"],"output":output}),
    );
}
fn discovery(task: &Value) -> Value {
    let mut finding = candidate("worker");
    finding["references"] = json!(["v2:1", "v2:2", "repository:src/provider.rs"]);
    if task["role"] == "Graph" {
        finding["files"] = json!([]);
        finding["identifiers"] = json!([]);
        finding["keyword_groups"] = json!([]);
        finding["recommendation"] = json!(
            "Check cached OAuth credentials before retrying a rejected provider guard repair."
        );
    }
    json!({"lessons":[finding]})
}
// Deliberately exercise compatibility with response files produced by earlier task schemas.
fn maintenance(lesson: &Value, action: &str) -> Value {
    let references = match lesson["scope"]["kind"].as_str().assert_value() {
        "graph" => json!(["graph:worker", "node:worker"]),
        _ => json!(["repository:src/provider.rs", "node:worker"]),
    };
    json!({"id":lesson["id"],"revision":lesson["revision"],"action":action,"title":"","recommendation":"",
        "files":[],"folders":[],"identifiers":[],"keyword_groups":[],"references":references,
        "reason":"The current authority still implements the credential check described by the entire supplied recommendation."})
}

#[tokio::test]
async fn initialization_requires_final_reviews_and_keeps_history_unchanged() {
    let case = case().await;
    let ledger = case
        .state
        .join("runs")
        .join(case.stored.snapshot.run_id.as_str())
        .join("runs.sqlite3");
    let before = std::fs::read(&ledger).assert_value();
    let inventory = case.prepare().await;
    assert_eq!(inventory["runs"].as_array().assert_value().len(), 1);
    assert_eq!(
        inventory["runs"][0]["tasks"]
            .as_array()
            .assert_value()
            .len(),
        2
    );
    assert!(
        initialize::review(&case.session).await.is_err(),
        "unclassified history must not be accepted"
    );
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let review = initialize::review(&case.session).await.assert_value();
    assert_eq!(review["staged_new_lessons"], 2);
    let state = read(&case.session.join("review.json"));
    for lesson in state["snapshot"]["lessons"].as_array().assert_value() {
        assert!(
            case.store()
                .record(lesson["id"].as_str().assert_value())
                .assert_value()
                .is_none()
        );
    }
    assert!(
        initialize::commit(&case.session).await.is_err(),
        "maintenance cannot be omitted"
    );
    case.maintenance(&review, "keep");
    let report = initialize::commit(&case.session).await.assert_value();
    assert_eq!(report["published_mutations"], 2);
    assert_eq!(std::fs::read(&ledger).assert_value(), before);
    assert_eq!(
        initialize::commit(&case.session).await.assert_value(),
        report,
        "an interrupted caller can recover the same publication receipt"
    );
}

#[tokio::test]
async fn initialization_does_not_publish_deferred_or_retired_new_lessons() {
    for action in ["defer", "retire"] {
        let case = case().await;
        let review = case.stage().await;
        case.maintenance(&review, action);
        let report = initialize::commit(&case.session).await.assert_value();
        assert_eq!(report["published_mutations"], 0);
        if action == "defer" {
            assert_eq!(report["deferred"], 2);
        }
    }
}

#[tokio::test]
async fn initialization_rejects_stale_source_and_stale_response_digests() {
    let case = case().await;
    let review = case.stage().await;
    case.maintenance(&review, "keep");
    std::fs::write(
        case.workspace().join("src/provider.rs"),
        "fn reject_provider_controls() { changed(); }\n",
    )
    .assert_value();
    assert!(initialize::commit(&case.session).await.is_err());
    let updated = initialize::review(&case.session).await.assert_value();
    assert_ne!(review["source_digest"], updated["source_digest"]);
    assert!(
        initialize::commit(&case.session).await.is_err(),
        "old response files must not satisfy a new review"
    );
    case.maintenance(&updated, "keep");
    assert_eq!(
        initialize::commit(&case.session).await.assert_value()["published_mutations"],
        2
    );
}

#[tokio::test]
async fn initialization_rejects_incomplete_or_cross_scope_maintenance() {
    let case = case().await;
    let review = case.stage().await;
    case.maintenance(&review, "keep");
    let tasks = review["maintenance_tasks"].as_array().assert_value();
    response(&case.session, &tasks[0]["task"], json!({"decisions":[]}));
    assert!(initialize::commit(&case.session).await.is_err());
    let other = read(&case.session.join(format!(
        "{}.response.json",
        tasks[1]["task"]["path"].as_str().assert_value()
    )));
    response(&case.session, &tasks[0]["task"], other["output"].clone());
    assert!(initialize::commit(&case.session).await.is_err());
}

#[tokio::test]
async fn initialization_respects_opt_out_and_explicit_exclusions() {
    let mut case = case().await;
    let mut opted_out = case.stored.clone();
    opted_out.snapshot.run_id = RunId::new(uuid::Uuid::now_v7().to_string());
    opted_out.admitted.runtime.active_memory_mut().repository = false;
    opted_out.admitted.runtime.active_memory_mut().graph = false;
    write_history(&case.state, &case.workspace(), &opted_out);
    let mut unfinished = case.stored.clone();
    unfinished.snapshot.run_id = RunId::new(uuid::Uuid::now_v7().to_string());
    unfinished.snapshot.phase = crate::v2_run_ledger::RunPhase::Running;
    unfinished.snapshot.terminal = None;
    write_history(&case.state, &case.workspace(), &unfinished);
    let mut foreign = case.stored.clone();
    foreign.snapshot.run_id = RunId::new(uuid::Uuid::now_v7().to_string());
    write_history(
        &case.state,
        &case.workspace().join("another-checkout"),
        &foreign,
    );
    let inventory = case.prepare().await;
    assert_eq!(inventory["runs"].as_array().assert_value().len(), 1);
    assert_eq!(inventory["excluded"].as_array().assert_value().len(), 3);
    write(
        &case.session.join("selection.json"),
        &json!({"runs":[{"run_id":case.stored.snapshot.run_id,
        "include":false,"reason":"Autoresearch is outside this initialization"}]}),
    );
    assert!(initialize::review(&case.session).await.is_err());
    case.stored.admitted.runtime.active_memory_mut().graph = false;
    write_history(&case.state, &case.workspace(), &case.stored);
    case.session = case.fixture._directory.path().join("repo-only-session");
    let inventory = case.prepare().await;
    assert_eq!(
        inventory["runs"][0]["tasks"]
            .as_array()
            .assert_value()
            .len(),
        1
    );
    assert_eq!(inventory["runs"][0]["tasks"][0]["role"], "Repository");
}

#[tokio::test]
async fn initialization_deduplicates_repeated_findings_and_respects_tombstones() {
    let case = case().await;
    let mut second = case.stored.clone();
    second.snapshot.run_id = RunId::new(uuid::Uuid::now_v7().to_string());
    write_history(&case.state, &case.workspace(), &second);
    let inventory = case.prepare().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let first = initialize::review(&case.session).await.assert_value();
    assert_eq!(
        first["staged_new_lessons"], 2,
        "equivalent discoveries from both runs collapse"
    );
    case.maintenance(&first, "keep");
    initialize::commit(&case.session).await.assert_value();
    let state = read(&case.session.join("review.json"));
    let retired = state["snapshot"]["lessons"][0]["id"]
        .as_str()
        .assert_value();
    case.store()
        .apply_batch(&[LessonMutation::Delete {
            id: retired.to_owned(),
            expected_revision: 1,
        }])
        .assert_value();
    let repeated = initialize::review(&case.session).await.assert_value();
    assert_eq!(repeated["staged_new_lessons"], 0);
    assert_eq!(repeated["lessons_to_reconcile"], 1);
}

#[tokio::test]
async fn initialization_cas_also_protects_unchanged_existing_lessons() {
    let case = case().await;
    let inventory = case.prepare().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let first = initialize::review(&case.session).await.assert_value();
    case.maintenance(&first, "keep");
    initialize::commit(&case.session).await.assert_value();
    let second = initialize::review(&case.session).await.assert_value();
    case.maintenance(&second, "keep");
    let state = read(&case.session.join("review.json"));
    let id = state["snapshot"]["lessons"][0]["id"]
        .as_str()
        .assert_value();
    case.store()
        .apply_batch(&[LessonMutation::Delete {
            id: id.to_owned(),
            expected_revision: 1,
        }])
        .assert_value();
    assert!(
        initialize::commit(&case.session).await.is_err(),
        "a concurrent retirement invalidates even a keep-only review"
    );
}

#[tokio::test]
async fn initialization_proves_remote_history_identity_from_a_detached_checkout() {
    let case = history_identity::source_case(true).await;
    git(&case.workspace(), &["checkout", "--quiet", "--detach"]);
    let inventory = case.prepare().await;
    assert_eq!(inventory["runs"].as_array().assert_value().len(), 1);
    let aliases = inventory["runs"][0]["repository_aliases"]
        .as_array()
        .assert_value();
    assert!(aliases.contains(&json!("example/memory-fixture")));
    assert!(aliases.contains(&json!(case.workspace())));
    assert!(inventory["excluded"].as_array().assert_value().is_empty());
}

#[tokio::test]
async fn initialization_adapts_new_lessons_then_retires_existing_versions() {
    let case = case().await;
    let review = case.stage().await;
    case.reconcile(&review, |lesson| {
        let mut result = maintenance(lesson, "adapt");
        result["title"] = json!("Updated provider guard procedure");
        result["recommendation"] =
            json!("Check stored refresh credentials before retrying a provider guard change.");
        if lesson["scope"]["kind"] == "repository" {
            result["files"] = json!(["src/provider.rs"]);
        }
        result
    });
    assert_eq!(
        initialize::commit(&case.session).await.assert_value()["published_mutations"],
        2
    );
    let state = read(&case.session.join("review.json"));
    for lesson in state["snapshot"]["lessons"].as_array().assert_value() {
        let stored = case
            .store()
            .record(lesson["id"].as_str().assert_value())
            .assert_value()
            .assert_value();
        assert_eq!(stored.version.revision, 1);
        assert_eq!(
            stored.lesson.assert_value().title,
            "Updated provider guard procedure"
        );
    }
    let second = initialize::review(&case.session).await.assert_value();
    case.maintenance(&second, "retire");
    assert_eq!(
        initialize::commit(&case.session).await.assert_value()["published_mutations"],
        2
    );
    for lesson in state["snapshot"]["lessons"].as_array().assert_value() {
        let stored = case
            .store()
            .record(lesson["id"].as_str().assert_value())
            .assert_value()
            .assert_value();
        assert!(stored.lesson.is_none());
        assert_eq!(stored.version.revision, 2);
    }
}

#[tokio::test]
async fn initialization_reports_one_corrupt_history_without_losing_other_runs() {
    let case = case().await;
    let mut corrupt = case.stored.clone();
    corrupt.snapshot.run_id = RunId::new(uuid::Uuid::now_v7().to_string());
    write_history(&case.state, &case.workspace(), &corrupt);
    let path = case
        .state
        .join("runs")
        .join(corrupt.snapshot.run_id.as_str())
        .join("runs.sqlite3");
    rusqlite::Connection::open(path)
        .assert_value()
        .execute(
            "INSERT INTO v2_run_events(run_id,sequence,event_json) VALUES(?1,1,'{invalid-json')",
            [corrupt.snapshot.run_id.as_str()],
        )
        .assert_value();
    let inventory = case.prepare().await;
    assert_eq!(inventory["runs"].as_array().assert_value().len(), 1);
    assert_eq!(inventory["excluded"].as_array().assert_value().len(), 1);
    assert_eq!(
        inventory["excluded"][0]["run_id"],
        corrupt.snapshot.run_id.as_str()
    );
    case.select(&inventory);
    case.discoveries(&inventory, false);
    let review = initialize::review(&case.session).await.assert_value();
    case.maintenance(&review, "keep");
    assert_eq!(
        initialize::commit(&case.session).await.assert_value()["published_mutations"],
        0
    );
}

#[tokio::test]
async fn one_init_command_resumes_missing_work_and_returns_its_completion_receipt() {
    let case = case().await;
    let inventory = case.advance().await;
    assert_eq!(inventory["phase"], "discovery");
    assert_eq!(inventory["next_action"], "select_runs");
    case.select(&inventory);
    let discovery_work = case.advance().await;
    assert_eq!(
        discovery_work["pending_tasks"]
            .as_array()
            .assert_value()
            .len(),
        2
    );
    let task = &discovery_work["pending_tasks"][0]["task"];
    response(&case.session, task, discovery(task));
    let resumed = case.advance().await;
    assert_eq!(resumed["pending_tasks"].as_array().assert_value().len(), 1);
    case.discoveries(&inventory, true);
    let review = case.advance().await;
    assert_eq!(review["phase"], "maintenance");
    assert_eq!(review["staged_new_lessons"], 2);
    assert_eq!(case.advance().await["phase"], "maintenance");
    case.maintenance(&review, "keep");
    let report = case.advance().await;
    assert_eq!(report["phase"], "complete");
    assert_eq!(report["published_mutations"], 2);
    std::fs::write(
        case.workspace().join("src/provider.rs"),
        "changed after completed initialization\n",
    )
    .assert_value();
    assert_eq!(
        case.advance().await,
        report,
        "a completed receipt must not replay even if current code changes"
    );
    for version in report["versions"].as_array().assert_value() {
        assert_eq!(
            case.store()
                .record(version["id"].as_str().assert_value())
                .assert_value()
                .assert_value()
                .version
                .revision,
            1
        );
    }
}

#[tokio::test]
async fn one_init_command_refreshes_maintenance_after_source_changes() {
    let case = case().await;
    let inventory = case.advance().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let initial = case.advance().await;
    case.maintenance(&initial, "keep");
    std::fs::write(
        case.workspace().join("src/provider.rs"),
        "fn reject_provider_controls() { changed(); }\n",
    )
    .assert_value();
    let refreshed = case.advance().await;
    assert_eq!(refreshed["phase"], "maintenance");
    assert_eq!(refreshed["refresh_reason"], "source_or_lessons_changed");
    assert_ne!(initial["source_digest"], refreshed["source_digest"]);
    assert_eq!(
        case.advance().await["phase"],
        "maintenance",
        "old response digests cannot finish fresh review"
    );
    case.maintenance(&refreshed, "keep");
    assert_eq!(case.advance().await["phase"], "complete");
}

#[tokio::test]
async fn one_init_command_refreshes_concurrently_changed_lessons() {
    let mut case = case().await;
    let inventory = case.advance().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let first = case.advance().await;
    case.maintenance(&first, "keep");
    let published = case.advance().await;
    case.session = case.fixture._directory.path().join("second-init");
    let inventory = case.advance().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let review = case.advance().await;
    case.maintenance(&review, "keep");
    let id = published["versions"][0]["id"].as_str().assert_value();
    case.store()
        .apply_batch(&[LessonMutation::Delete {
            id: id.to_owned(),
            expected_revision: 1,
        }])
        .assert_value();
    let refreshed = case.advance().await;
    assert_eq!(refreshed["phase"], "maintenance");
    assert_eq!(refreshed["lessons_to_reconcile"], 1);
    case.maintenance(&refreshed, "keep");
    assert_eq!(case.advance().await["published_mutations"], 0);
}

#[tokio::test]
async fn one_init_command_recovers_publication_before_its_file_checkpoint() {
    let case = case().await;
    let inventory = case.advance().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let review = case.advance().await;
    case.maintenance(&review, "keep");
    let published = initialize::commit(&case.session).await.assert_value();
    let pending = read(&case.session.join("publication.json"));
    assert!(
        case.store()
            .initialization_receipt(pending["id"].as_str().assert_value())
            .assert_value()
            .is_some()
    );
    std::fs::remove_file(case.session.join("committed.json")).assert_value();
    std::fs::write(
        case.workspace().join("src/provider.rs"),
        "changed after database commit\n",
    )
    .assert_value();
    let recovered = case.advance().await;
    assert_eq!(recovered["phase"], "complete");
    assert_eq!(recovered["versions"], published["versions"]);
    assert!(
        case.store()
            .initialization_receipt(pending["id"].as_str().assert_value())
            .assert_value()
            .is_none()
    );
    assert_eq!(case.advance().await, recovered);
}

#[tokio::test]
async fn one_init_command_serializes_sessions_and_rejects_changed_ignore_lists() {
    let case = case().await;
    case.advance().await;
    let lock = std::fs::File::options()
        .read(true)
        .write(true)
        .open(case.session.join(".init.lock"))
        .assert_value();
    fs2::FileExt::lock_exclusive(&lock).assert_value();
    assert!(
        initialize::init(case.state.clone(), &case.session, Default::default())
            .await
            .is_err()
    );
    fs2::FileExt::unlock(&lock).assert_value();
    let filters = initialize::Ignore::new(&["ignored-repository".to_owned()], &[]).assert_value();
    assert!(
        initialize::init(case.state.clone(), &case.session, filters)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn one_init_command_respects_changed_selection_before_publication() {
    let case = case().await;
    let inventory = case.advance().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let review = case.advance().await;
    case.maintenance(&review, "keep");
    write(
        &case.session.join("selection.json"),
        &json!({"runs":[{"run_id":case.stored.snapshot.run_id,
        "include":false,"reason":"User excluded this graph"}]}),
    );
    let report = case.advance().await;
    assert_eq!(report["phase"], "complete");
    assert_eq!(report["published_mutations"], 0);
}

#[test]
fn memory_cli_exposes_only_the_resumable_init_contract() {
    use crate::native_v2_cli::{MemoryCommand, NativeV2CliCommand, parse_native_v2_args};
    let parse = |args: &[&str]| parse_native_v2_args(args.iter().map(std::ffi::OsString::from));
    assert_eq!(
        parse(&["memory", "init", "--directory", "session"]).assert_value(),
        NativeV2CliCommand::Memory(MemoryCommand::Init {
            ignore_repos: Vec::new(),
            ignore_graphs: Vec::new(),
            directory: "session".into()
        })
    );
    for args in [
        vec!["memory", "review", "--directory", "session"],
        vec!["memory", "commit", "--directory", "session"],
        vec!["memory", "init", "--output", "session"],
        vec![
            "memory",
            "init",
            "--directory",
            "session",
            "--cwd",
            "repository",
        ],
        vec![
            "memory",
            "init",
            "--directory",
            "session",
            "--target",
            "cloud",
        ],
    ] {
        assert!(parse(&args).is_err());
    }
}

#[tokio::test]
async fn one_init_command_recovers_interrupted_preparation_from_saved_filters() {
    let case = case().await;
    let inventory = case.advance().await;
    std::fs::remove_file(case.session.join("prepared.json")).assert_value();
    std::fs::remove_file(case.session.join("session.json")).assert_value();
    let resumed = initialize::init(case.state.clone(), &case.session, Default::default())
        .await
        .assert_value();
    assert_eq!(resumed["phase"], "discovery");
    assert_eq!(resumed["filters"], inventory["filters"]);
    assert_eq!(resumed["runs"], inventory["runs"]);
    assert!(case.session.join("prepared.json").is_file());
}

#[tokio::test]
async fn refreshed_maintenance_can_retire_a_staged_lesson_after_its_source_was_renamed() {
    let case = case().await;
    let inventory = case.advance().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    case.advance().await;
    std::fs::rename(
        case.workspace().join("src/provider.rs"),
        case.workspace().join("src/current.rs"),
    )
    .assert_value();
    let refreshed = case.advance().await;
    assert_eq!(refreshed["phase"], "maintenance");
    assert_eq!(refreshed["staged_new_lessons"], 2);
    case.reconcile(&refreshed, |lesson| {
        if lesson["scope"]["kind"] == "repository" {
            let mut decision = maintenance(lesson, "retire");
            decision["references"] = json!(["node:worker", "repository:src/current.rs"]);
            decision["reason"] = json!("Current src/current.rs replaced the associated path, so the complete old path-specific recommendation no longer applies.");
            decision
        } else { maintenance(lesson, "keep") }
    });
    assert_eq!(case.advance().await["published_mutations"], 1);
}

#[tokio::test]
async fn late_preparation_cannot_overwrite_already_classified_discovery() {
    let case = case().await;
    let inventory = case.advance().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let selection = std::fs::read(case.session.join("selection.json")).assert_value();
    let repeated = case.prepare().await;
    assert_eq!(repeated["runs"], inventory["runs"]);
    assert_eq!(
        std::fs::read(case.session.join("selection.json")).assert_value(),
        selection
    );
    assert_eq!(case.advance().await["phase"], "maintenance");
}

#[path = "initialization/global.rs"]
mod global;

#[path = "initialization/history_identity.rs"]
mod history_identity;

#[tokio::test]
async fn legacy_response_revisions_cannot_select_a_newer_version_or_publish_partially() {
    let case = case().await;
    let review = case.stage().await;
    let before = case
        .store()
        .snapshot(&case.fixture.query)
        .assert_value()
        .generation;
    case.reconcile(&review, |lesson| {
        let mut result = maintenance(lesson, "keep");
        result["revision"] = json!(lesson["revision"].as_u64().assert_value() + 1);
        result
    });
    assert!(initialize::commit(&case.session).await.is_err());
    let after = case
        .store()
        .snapshot(&case.fixture.query)
        .assert_value()
        .generation;
    assert_eq!(
        after, before,
        "a mismatched legacy precondition must not publish any scope"
    );
}
