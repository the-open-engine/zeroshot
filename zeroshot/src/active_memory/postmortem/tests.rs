use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;

use openengine_cluster_protocol::{
    ActiveMemoryConfig, IdempotencyKey, Sha256Digest, TokenCount, WorkerOutcome,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::*;

mod diagnostics;
mod initialization;
mod reconciliation;
mod replay;
mod scoped_maintenance;
mod source_positions;
use crate::active_memory::{
    LessonDraft, LessonEvidence, LessonMutation, LessonScope, ScopeQuery, ToolObservation,
};
use crate::native_v2_admission::NativeV2Admission;
use crate::native_v2_contract::{NodeInvocation, RunSubmission, TokenUsageDelta};
use crate::native_v2_runner::test_support::CountingSession;
use crate::native_v2_runner::{
    DriverControl, DriverInvocation, LiveOutput, LiveOutputStream, NativeNodeRunner, NodeDriver,
    NodeRole, NodeSession, ResolvedEnvironment, SessionFactory, AUXILIARY_PROVIDER_SLOT_OFFSET,
};
use crate::native_v2_supervisor::NativeV2Supervisor;
use crate::v2_run_ledger::{CreateRun, RunEvent};
use crate::v2_run_ledger::fake::FakeRunLedger;

struct Source {
    manifest: Option<SuppliedMemoryManifest>,
    observations: StdMutex<Vec<ToolObservation>>,
}
impl SuppliedLessonSource for Source {
    fn manifest(&self) -> Option<SuppliedMemoryManifest> {
        self.manifest.clone()
    }
    fn tool_observations(&self) -> Vec<ToolObservation> {
        self.observations.lock().assert_value().clone()
    }
}

#[derive(Default)]
struct Driver {
    outputs: StdMutex<BTreeMap<String, Value>>,
    slots: StdMutex<Vec<(String, u64)>>,
    globals: AtomicUsize,
    closed: Arc<AtomicUsize>,
    worker_message: StdMutex<Option<String>>,
    maintenance_action: StdMutex<Option<&'static str>>,
    maintenance_omit: StdMutex<Option<String>>,
    maintenance_inputs: StdMutex<Vec<Value>>,
}

#[async_trait]
impl SessionFactory for Driver {
    async fn open(
        &self,
        _: &NodeInvocation,
        _: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        Ok(Arc::new(CountingSession {
            closed: self.closed.clone(),
        }))
    }
}

#[async_trait]
impl NodeDriver for Driver {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let name = invocation.node.reference.node.as_str().to_owned();
        self.slots
            .lock()
            .assert_value()
            .push((name.clone(), invocation.provider_session_slot.get()));
        control
            .record_token_usage(Some(TokenUsageDelta {
                input_tokens: TokenCount::new(1).assert_value(),
                output_tokens: TokenCount::new(1).assert_value(),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            }))
            .await?;
        if invocation.role == NodeRole::Worker {
            let text=self.worker_message.lock().assert_value().clone().unwrap_or_else(||
                "A costly rejected provider guard was repaired after the initial focused tests omitted OAuth controls.".to_owned());
            control
                .emit(LiveOutput::new(LiveOutputStream::System, text)?)
                .await?;
            return Ok(WorkerOutcome::Verified {
                output: Value::Null,
                artifacts: Vec::new(),
            });
        }
        self.globals.fetch_add(1, Ordering::SeqCst);
        let fallback = if name.ends_with("_maintenance") {
            self.maintenance_output(&invocation)
        } else {
            json!({"lessons":[]})
        };
        let output = self
            .outputs
            .lock()
            .assert_value()
            .get(&name)
            .cloned()
            .unwrap_or(fallback);
        Ok(WorkerOutcome::Verifier {
            output,
            signals: BTreeMap::new(),
            diagnostic: Value::Null,
            artifacts: Vec::new(),
        })
    }
}

impl Driver {
    fn maintenance_output(&self, invocation: &DriverInvocation) -> Value {
        let input: Value =
            serde_json::from_str(invocation.node.input["context"].as_str().assert_value())
                .assert_value();
        self.maintenance_inputs
            .lock()
            .assert_value()
            .push(input.clone());
        let action = self
            .maintenance_action
            .lock()
            .assert_value()
            .unwrap_or("defer");
        let omitted = self.maintenance_omit.lock().assert_value().clone();
        let decisions = input["supplied_lessons"]
            .as_object()
            .assert_value()
            .iter()
            .filter(|(id, _)| omitted.as_deref() != Some(id.as_str()))
            .map(|(id, lesson)| {
                let mut output = decision(action);
                output["id"] = json!(id);
                if lesson["scope"]["kind"] == "graph" {
                    output["references"] = json!(["node:worker", "graph:worker"]);
                    for key in ["files", "folders", "identifiers", "keyword_groups"] {
                        output[key] = json!([]);
                    }
                }
                if action == "defer" {
                    output["title"] = json!("");
                    output["recommendation"] = json!("");
                    for key in [
                        "files",
                        "folders",
                        "identifiers",
                        "keyword_groups",
                        "references",
                    ] {
                        output[key] = json!([]);
                    }
                }
                output
            })
            .collect::<Vec<_>>();
        json!({"decisions":decisions})
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    supervisor: NativeV2Supervisor,
    ledger: Arc<FakeRunLedger>,
    store: Arc<Mutex<ActiveMemoryStore>>,
    driver: Arc<Driver>,
    factory: Arc<dyn AuxiliaryRunnerFactory>,
    admitted: AdmittedRun,
    run_id: RunId,
    query: ScopeQuery,
    source: Arc<Source>,
}

impl Fixture {
    fn substantial_exploration(&self) {
        *self.source.observations.lock().assert_value() = [1, 150001, 300001]
            .into_iter()
            .enumerate()
            .map(|(index, timestamp_ms)| ToolObservation {
                seq: index as u64 + 1,
                node_id: "worker".to_owned(),
                execution_id: 1,
                timestamp_ms,
                tool_name: "read_file".to_owned(),
                paths: vec!["src/provider.rs".to_owned()],
            })
            .collect();
    }

    fn output(&self, role: &str, output: Value) {
        self.driver
            .outputs
            .lock()
            .assert_value()
            .insert(format!("active_memory_{role}"), output);
    }

    async fn lessons(&self) -> Vec<crate::active_memory::Lesson> {
        self.store
            .lock()
            .await
            .snapshot(&self.query)
            .assert_value()
            .lessons
    }

    async fn drive(&self) {
        assert!(matches!(
            self.supervisor.drive().await.assert_value(),
            TerminalResult::Succeeded { .. }
        ));
    }

    async fn postmortem_request(&self) -> PostmortemRequest {
        let stored = self
            .ledger
            .get(&self.run_id)
            .await
            .assert_value()
            .assert_value();
        PostmortemRequest {
            run_id: self.run_id.clone(),
            admitted: self.admitted.clone(),
            primary_terminal: stored.snapshot.terminal.clone().assert_value(),
            snapshot: stored.snapshot,
            environment: Arc::new(
                RunEnvironment::exact_for_admitted(&self.admitted, BTreeMap::new()).assert_value(),
            ),
            ledger: self.ledger.clone(),
        }
    }

    async fn maintenance_rejected(&self) -> bool {
        self.ledger.snapshot_and_tail(&self.run_id,None).await.assert_value().events.iter().any(|event| {
            matches!(&event.event,RunEvent::SafeLog {line,..} if line.as_str().contains("maintenance findings rejected"))
        })
    }
}

async fn fixture(
    settings: ActiveMemoryConfig,
    seed: Option<LessonDraft>,
    manifest_known: bool,
) -> Fixture {
    fixture_with_lessons(settings, seed.into_iter().collect(), manifest_known).await
}

async fn fixture_with_lessons(
    settings: ActiveMemoryConfig,
    seeds: Vec<LessonDraft>,
    manifest_known: bool,
) -> Fixture {
    let directory = tempfile::tempdir().assert_value();
    let workspace = directory.path().join("workspace");
    std::fs::create_dir_all(workspace.join("src")).assert_value();
    std::fs::write(
        workspace.join("src/provider.rs"),
        "fn reject_provider_controls() {}\n",
    )
    .assert_value();
    let submission:RunSubmission=serde_json::from_value(json!({
        "title":"Active Memory test", "submissionKey":"active-memory-test", "initialInput":null,
        "graph":{"profile":"openengine.graph.full/v1","initialInput":{"kind":"null"},
            "policy":{"policy":"policy.native-v2@1","default":"deny"},
            "root":{"kind":"seq","name":"root","state":{"kind":"null"},"promotedStatePaths":[],"children":[
                {"kind":"step","name":"worker","worker":"worker.test@1","instructions":"Implement the task.",
                    "input":{"kind":"null"},"output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1},
                {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
            ]}},
        "runtime":{"harness":"codex","provider":"openai","size":"small","activeMemory":settings,
            "nodes":{"worker":{"kind":"agent","model":"opaque-test-model","connections":{},"sessionScope":"node_instance"}}}
    })).assert_value();
    let admitted = NativeV2Admission.admit(submission).await.assert_value();
    let run_id = RunId::new("active-memory-run");
    let ledger = Arc::new(FakeRunLedger::new());
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("active-memory-test").assert_value(),
            submission_digest: Sha256Digest::new("0".repeat(64)).assert_value(),
            admitted: admitted.clone(),
        })
        .await
        .assert_value();
    let driver = Arc::new(Driver::default());
    let runner =
        Arc::new(NativeNodeRunner::new(&admitted, driver.clone(), driver.clone()).assert_value());
    let factory = runner.auxiliary_factory();
    let store = Arc::new(Mutex::new(
        ActiveMemoryStore::open(&directory.path().join("memory/lessons.sqlite")).assert_value(),
    ));
    let mutations = seeds
        .into_iter()
        .map(|draft| LessonMutation::Upsert {
            expected_revision: None,
            draft: Box::new(draft),
        })
        .collect::<Vec<_>>();
    store.lock().await.apply_batch(&mutations).assert_value();
    let graph_hash = admitted.graph.identity().assert_value().as_str().to_owned();
    let query = ScopeQuery {
        repository: Some("example/repo".to_owned()),
        graph_hash: Some(graph_hash.clone()),
        node_id: None,
        repository_lessons: true,
        graph_lessons: true,
    };
    let snapshot = store.lock().await.snapshot(&query).assert_value();
    let manifest = manifest_known.then(|| SuppliedMemoryManifest {
        lessons: snapshot
            .lessons
            .iter()
            .map(crate::active_memory::Lesson::version)
            .collect(),
    });
    let source = Arc::new(Source {
        manifest,
        observations: StdMutex::new(Vec::new()),
    });
    let postmortem = Arc::new(BuiltinPostmortem::new(BuiltinPostmortemConfig {
        store: store.clone(),
        factory: factory.clone(),
        snapshot,
        supplied: source.clone(),
        repository: query.repository.clone(),
        graph_hash,
        workspace,
    }));
    let environment =
        Arc::new(RunEnvironment::exact_for_admitted(&admitted, BTreeMap::new()).assert_value());
    let supervisor = NativeV2Supervisor::new(run_id.clone(), ledger.clone(), runner, environment)
        .with_postmortem(Some(postmortem));
    Fixture {
        _directory: directory,
        supervisor,
        ledger,
        store,
        driver,
        factory,
        admitted,
        run_id,
        query,
        source,
    }
}

fn candidate(node: &str) -> Value {
    json!({"title":"Keep provider controls complete","recommendation":"Reject OAuth refresh tokens as well as the focused API-key controls before launching Bedrock.",
        "node_id":node,"files":["src/provider.rs"],"folders":[],"identifiers":["reject_provider_controls"],"keyword_groups":[["oauth","bedrock"]],
        "references":["hook:1","hook:2","hook:3","repository:src/provider.rs"],"costly_recurring_reason":"Sustained provider exploration established the otherwise non-obvious credential interaction needed by future lane changes.","repository_specific":true})
}

fn seed_lesson() -> LessonDraft {
    LessonDraft {
        id: "existing-provider-lesson".to_owned(),
        scope: LessonScope::Repository {
            repository: "example/repo".to_owned(),
            files: vec!["src/provider.rs".to_owned()],
            folders: Vec::new(),
            identifiers: Vec::new(),
            keyword_groups: Vec::new(),
        },
        title: "Existing provider lesson".to_owned(),
        recommendation: "Check all provider controls.".to_owned(),
        evidence: LessonEvidence {
            run_id: "older-run".to_owned(),
            node_id: "worker".to_owned(),
            references: vec!["older-evidence".to_owned()],
            costly_recurring_reason:
                "An earlier rejected repair omitted adjacent provider controls.".to_owned(),
        },
    }
}

fn decision(action: &str) -> Value {
    json!({"id":"existing-provider-lesson","action":action,"title":"Updated provider lesson",
        "recommendation":"Reject every cached OAuth credential before using the Bedrock lane.","files":["src/provider.rs"],"folders":[],"identifiers":[],"keyword_groups":[],
        "references":["node:worker","repository:src/provider.rs"],"reason":"Current source and this run show the old advice omitted a recurring cached OAuth credential failure."})
}

fn ledger_format_lesson() -> LessonDraft {
    let mut lesson = seed_lesson();
    lesson.title = "Use the durable ledger format".to_owned();
    lesson.recommendation = "Use ledgerFormat 3 in settings/schema.json.".to_owned();
    lesson.scope = LessonScope::Repository {
        repository: "example/repo".to_owned(),
        files: vec!["settings/schema.json".to_owned()],
        folders: Vec::new(),
        identifiers: Vec::new(),
        keyword_groups: Vec::new(),
    };
    lesson
}

fn write_ledger_format(fixture: &Fixture, version: u32) {
    let settings = fixture._directory.path().join("workspace/settings");
    std::fs::create_dir_all(&settings).assert_value();
    std::fs::write(
        settings.join("schema.json"),
        format!("{{\"ledgerFormat\":{version}}}\n"),
    )
    .assert_value();
}

#[tokio::test]
async fn repository_keep_rejects_circular_confirmation_from_a_repeated_stale_card() {
    let fixture = self::fixture(Default::default(), Some(ledger_format_lesson()), true).await;
    write_ledger_format(&fixture, 4);
    *fixture.driver.worker_message.lock().assert_value() =
        Some("The supplied memory says to use ledgerFormat 3 in settings/schema.json.".to_owned());
    let mut keep = decision("keep");
    keep["references"] = json!(["node:worker"]);
    keep["reason"] =
        json!("The main worker repeated ledgerFormat 3, so the supplied advice is still valid.");
    fixture.output("repository_maintenance", json!({"decisions":[keep]}));
    fixture.drive().await;
    assert!(fixture.maintenance_rejected().await);
    assert_eq!(fixture.lessons().await[0].revision, 1);
}

#[tokio::test]
async fn repository_keep_accepts_a_current_file_citation_and_explicit_comparison() {
    let fixture = self::fixture(Default::default(), Some(ledger_format_lesson()), true).await;
    write_ledger_format(&fixture, 3);
    let mut keep = decision("keep");
    keep["references"] = json!(["repository:settings/schema.json"]);
    keep["reason"] = json!(
        "Current settings/schema.json declares ledgerFormat 3, matching the supplied lesson's concrete version."
    );
    fixture.output("repository_maintenance", json!({"decisions":[keep]}));
    fixture.drive().await;
    assert!(!fixture.maintenance_rejected().await);
    assert_eq!(fixture.lessons().await[0].revision, 1);
}

#[tokio::test]
async fn maintenance_current_file_gates_reject_missing_paths_and_empty_justification() {
    for (reference, reason) in [
        (
            "repository:missing.rs",
            "Current source confirms the advice.",
        ),
        (
            "repository:../outside.rs",
            "Current source confirms the advice.",
        ),
        ("repository:src/provider.rs", ""),
    ] {
        let fixture = self::fixture(Default::default(), Some(seed_lesson()), true).await;
        let mut keep = decision("keep");
        keep["references"] = json!([reference]);
        keep["reason"] = json!(reason);
        fixture.output("repository_maintenance", json!({"decisions":[keep]}));
        fixture.drive().await;
        assert!(fixture.maintenance_rejected().await);
    }
}

#[tokio::test]
async fn maintenance_adapt_and_retire_require_current_source_as_well_as_run_evidence() {
    for action in ["adapt", "retire"] {
        for references in [
            json!(["node:worker"]),
            json!(["repository:src/provider.rs"]),
        ] {
            let fixture = self::fixture(Default::default(), Some(seed_lesson()), true).await;
            let mut mutation = decision(action);
            mutation["references"] = references;
            fixture.output("repository_maintenance", json!({"decisions":[mutation]}));
            fixture.drive().await;
            assert!(fixture.maintenance_rejected().await);
            assert_eq!(fixture.lessons().await[0].revision, 1);
        }
    }
}

#[tokio::test]
async fn graph_keep_requires_the_current_authored_node_reference() {
    let template = self::fixture(Default::default(), None, true).await;
    let mut lesson = seed_lesson();
    lesson.scope = LessonScope::Graph {
        graph_hash: template.query.graph_hash.clone().assert_value(),
        node_id: "worker".to_owned(),
        repository: None,
    };
    for (reference, rejected) in [
        ("node:worker", true),
        ("graph:worker", false),
        ("graph:invented_node", true),
    ] {
        let fixture = self::fixture(Default::default(), Some(lesson.clone()), true).await;
        let mut keep = decision("keep");
        keep["references"] = json!([reference]);
        keep["reason"] = json!(
            "The current authored worker node and its output contract retain the procedural operation covered by this lesson."
        );
        fixture.output("graph_maintenance", json!({"decisions":[keep]}));
        fixture.drive().await;
        assert_eq!(fixture.maintenance_rejected().await, rejected);
        assert_eq!(fixture.lessons().await[0].revision, 1);
    }
}

#[tokio::test]
async fn maintenance_can_defer_when_current_source_verification_is_unavailable() {
    let fixture = self::fixture(Default::default(), Some(seed_lesson()), true).await;
    let mut deferred = decision("defer");
    deferred["references"] = json!([]);
    deferred["reason"] =
        json!("Current authoritative source could not be verified, so no freshness claim is made.");
    fixture.output("repository_maintenance", json!({"decisions":[deferred]}));
    fixture.drive().await;
    assert!(!fixture.maintenance_rejected().await);
    assert_eq!(fixture.lessons().await[0].revision, 1);
}

#[tokio::test]
async fn four_specialists_share_run_authority_and_publish_scoped_lessons_without_reducer_executions()
 {
    let fixture = self::fixture(Default::default(), None, true).await;
    fixture.substantial_exploration();
    fixture.output("repository", json!({"lessons":[candidate("worker")]}));
    fixture.output("graph", json!({"lessons":[candidate("worker")]}));
    fixture.drive().await;
    assert_eq!(fixture.driver.globals.load(Ordering::SeqCst), 4);
    assert_eq!(fixture.driver.closed.load(Ordering::SeqCst), 5);
    let lessons = fixture.lessons().await;
    assert_eq!(lessons.len(), 1);
    assert!(
        lessons
            .iter()
            .any(|lesson| matches!(lesson.scope, LessonScope::Repository { .. }))
    );
    // Exploration alone establishes repository cost; graph guidance requires repeated repairs.
    let stored = fixture
        .ledger
        .get(&fixture.run_id)
        .await
        .assert_value()
        .assert_value();
    assert_eq!(stored.snapshot.executions.len(), 1);
    assert!(stored.snapshot.execution_seed.is_empty());
    assert_eq!(
        stored
            .snapshot
            .token_usage
            .as_ref()
            .assert_value()
            .output_tokens
            .get(),
        5
    );
    let slots = fixture.driver.slots.lock().assert_value();
    assert!(
        slots
            .iter()
            .filter(|(name, _)| name.starts_with("active_memory_"))
            .all(|(_, slot)| *slot > AUXILIARY_PROVIDER_SLOT_OFFSET)
    );
}

#[tokio::test]
async fn maintenance_binds_decision_ids_to_the_frozen_revision_and_the_host_increments_it() {
    let fixture = self::fixture(Default::default(), Some(seed_lesson()), true).await;
    fixture.output(
        "repository_maintenance",
        json!({"decisions":[decision("adapt")]}),
    );
    fixture.drive().await;
    let lessons = fixture.lessons().await;
    assert_eq!(lessons.len(), 1);
    assert_eq!(lessons[0].revision, 2);
    assert!(lessons[0].recommendation.contains("cached OAuth"));
}

#[tokio::test]
async fn unknown_supplied_manifest_prevents_maintenance_mutation() {
    let fixture = self::fixture(Default::default(), Some(seed_lesson()), false).await;
    fixture.output(
        "repository_maintenance",
        json!({"decisions":[decision("retire")]}),
    );
    fixture.drive().await;
    assert_eq!(fixture.lessons().await[0].revision, 1);
}

#[tokio::test]
async fn stale_maintenance_revision_cannot_overwrite_a_newer_lesson_or_publish_a_partial_batch() {
    let fixture = self::fixture(Default::default(), Some(seed_lesson()), true).await;
    fixture.substantial_exploration();
    let mut newer = seed_lesson();
    newer.recommendation = "A concurrent run already corrected the advice.".to_owned();
    fixture
        .store
        .lock()
        .await
        .apply_batch(&[LessonMutation::Upsert {
            expected_revision: Some(1),
            draft: Box::new(newer),
        }])
        .assert_value();
    fixture.output(
        "repository_maintenance",
        json!({"decisions":[decision("adapt")]}),
    );
    fixture.output("repository", json!({"lessons":[candidate("worker")]}));
    fixture.drive().await;
    let lessons = fixture.lessons().await;
    assert_eq!(lessons.len(), 1);
    assert_eq!(lessons[0].revision, 2);
    assert!(lessons[0].recommendation.contains("concurrent"));
}

#[tokio::test]
async fn invalid_discovery_node_is_rejected_while_other_specialists_can_succeed() {
    let fixture = self::fixture(Default::default(), None, true).await;
    fixture.substantial_exploration();
    fixture.output("graph", json!({"lessons":[candidate("invented_node")]}));
    fixture.output("repository", json!({"lessons":[candidate("worker")]}));
    fixture.drive().await;
    let lessons = fixture.lessons().await;
    assert_eq!(lessons.len(), 1);
    assert!(matches!(lessons[0].scope, LessonScope::Repository { .. }));
}

#[tokio::test]
async fn collection_respects_independent_scope_opt_outs_and_total_opt_out() {
    let fixture = self::fixture(
        ActiveMemoryConfig {
            graph: false,
            ..Default::default()
        },
        None,
        true,
    )
    .await;
    fixture.output("graph", json!({"lessons":[candidate("worker")]}));
    fixture.drive().await;
    assert_eq!(fixture.driver.globals.load(Ordering::SeqCst), 2);
    assert!(fixture.lessons().await.is_empty());
    let fixture = self::fixture(
        ActiveMemoryConfig {
            repository: false,
            graph: false,
            ..Default::default()
        },
        None,
        true,
    )
    .await;
    fixture.drive().await;
    assert_eq!(fixture.driver.globals.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn auxiliary_factory_rejects_forks_after_close() {
    let fixture = self::fixture(Default::default(), None, true).await;
    fixture.factory.close_run(&fixture.run_id).await;
    let plan = plan::build(&fixture.admitted).await.assert_value();
    assert!(matches!(
        fixture.factory.fork(&plan),
        Err(NodeRunnerError::RunClosed)
    ));
}

#[tokio::test]
async fn global_usage_is_recorded_separately_from_graph_execution_history() {
    let fixture = self::fixture(Default::default(), None, true).await;
    fixture.drive().await;
    let page = fixture
        .ledger
        .snapshot_and_tail(&fixture.run_id, None)
        .await
        .assert_value();
    assert_eq!(
        page.events
            .iter()
            .filter(|event| matches!(event.event, RunEvent::RunTokenUsageObserved { .. }))
            .count(),
        4
    );
}

#[tokio::test]
async fn reordered_associations_do_not_duplicate_an_existing_lesson() {
    let mut existing = seed_lesson();
    let mut proposed = candidate("worker");
    existing.recommendation = proposed["recommendation"]
        .as_str()
        .assert_value()
        .to_owned();
    existing.scope = LessonScope::Repository {
        repository: "example/repo".to_owned(),
        files: vec!["src/b.rs".to_owned(), "src/a.rs".to_owned()],
        folders: vec!["tests".to_owned(), "src".to_owned()],
        identifiers: vec!["ProviderBeta".to_owned(), "ProviderAlpha".to_owned()],
        keyword_groups: vec![
            vec!["oauth".to_owned(), "bedrock".to_owned()],
            vec!["selector".to_owned(), "gateway".to_owned()],
        ],
    };
    proposed["files"] = json!(["src/a.rs", "src/b.rs"]);
    proposed["folders"] = json!(["src", "tests"]);
    proposed["identifiers"] = json!(["ProviderAlpha", "ProviderBeta"]);
    proposed["keyword_groups"] = json!([["gateway", "selector"], ["bedrock", "oauth"]]);
    let fixture = self::fixture(Default::default(), Some(existing), true).await;
    fixture.substantial_exploration();
    fixture.output("repository", json!({"lessons":[proposed]}));
    fixture.drive().await;
    let lessons = fixture.lessons().await;
    assert_eq!(lessons.len(), 1);
    assert_eq!(lessons[0].id, "existing-provider-lesson");
    assert_eq!(lessons[0].revision, 1);
}

#[tokio::test]
async fn one_ordinary_output_correction_cannot_create_graph_or_repository_lessons() {
    let fixture = self::fixture(Default::default(), None, true).await;
    *fixture.driver.worker_message.lock().assert_value() = Some(
        "Final output rejected once; correction returned the required bare JSON object.".to_owned(),
    );
    let mut ordinary = candidate("worker");
    ordinary["references"] = json!(["node:worker", "repository:src/provider.rs"]);
    ordinary["title"] = json!("Return bare JSON");
    ordinary["recommendation"] = json!("Return the required JSON object without Markdown fences.");
    fixture.output("graph", json!({"lessons":[ordinary.clone()]}));
    fixture.output("repository", json!({"lessons":[ordinary]}));
    fixture.drive().await;
    assert!(fixture.lessons().await.is_empty());
}

#[tokio::test]
async fn repository_discovery_requires_current_source_and_concrete_verified_associations() {
    for (files, folders, identifiers, references, accepted) in [
        (
            json!([]),
            json!([]),
            json!([]),
            json!(["hook:1", "hook:2", "hook:3", "repository:src/provider.rs"]),
            false,
        ),
        (
            json!([]),
            json!([]),
            json!(["invented_identifier"]),
            json!(["hook:1", "hook:2", "hook:3", "repository:src/provider.rs"]),
            false,
        ),
        (
            json!(["src/provider.rs"]),
            json!([]),
            json!([]),
            json!(["hook:1", "hook:2", "hook:3"]),
            false,
        ),
        (
            json!([]),
            json!([]),
            json!(["reject_provider_controls"]),
            json!(["hook:1", "hook:2", "hook:3", "repository:src/provider.rs"]),
            true,
        ),
    ] {
        let fixture = self::fixture(Default::default(), None, true).await;
        fixture.substantial_exploration();
        let mut proposed = candidate("worker");
        proposed["files"] = files;
        proposed["folders"] = folders;
        proposed["identifiers"] = identifiers;
        proposed["references"] = references;
        fixture.output("repository", json!({"lessons":[proposed]}));
        fixture.drive().await;
        assert_eq!(fixture.lessons().await.len(), usize::from(accepted));
    }
}

#[tokio::test]
async fn fewer_than_three_tools_or_short_exploration_cannot_claim_high_impact() {
    for timestamps in [vec![1, 300001], vec![1, 150001, 299999]] {
        let fixture = self::fixture(Default::default(), None, true).await;
        fixture.substantial_exploration();
        let observations = fixture.source.tool_observations();
        *fixture.source.observations.lock().assert_value() = observations
            .into_iter()
            .zip(timestamps)
            .map(|(mut observation, timestamp)| {
                observation.timestamp_ms = timestamp;
                observation
            })
            .collect();
        fixture.output("repository", json!({"lessons":[candidate("worker")]}));
        fixture.drive().await;
        assert!(fixture.lessons().await.is_empty());
    }
}

#[test]
fn specialist_prompts_distinguish_evidence_from_replacements_and_require_whole_advice_validity() {
    let maintenance = plan::Role::RepositoryMaintenance.instruction();
    assert!(maintenance.contains("ENTIRE recommendation"));
    assert!(maintenance.contains("Keep still REQUIRES nonempty references and reason"));
    assert!(
        plan::Role::Graph
            .instruction()
            .contains("Do not restate existing instructions or the response/output contract")
    );
}

#[tokio::test]
async fn repeated_failed_main_completions_are_eligible_but_malformed_outputs_are_not() {
    use crate::native_v2_contract::ExecutionId;
    use crate::v2_run_ledger::{NodeState, cursor_for};
    use openengine_cluster_protocol::{WorkerErrorCode, WorkerFailureReason};
    let fixture = self::fixture(Default::default(), None, true).await;
    fixture.drive().await;
    let mut request = fixture.postmortem_request().await;
    let template = request
        .snapshot
        .executions
        .values()
        .next()
        .assert_value()
        .clone();
    for (code, eligible) in [
        (WorkerErrorCode::Timeout, true),
        (WorkerErrorCode::Crash, true),
        (WorkerErrorCode::Malformed, false),
    ] {
        request.snapshot.executions.clear();
        for number in [1, 2] {
            let mut node = template.clone();
            node.reference.execution = ExecutionId::new(number).assert_value();
            node.state = NodeState::Completed {
                at: cursor_for(number),
                outcome: WorkerOutcome::Error {
                    code,
                    reason: WorkerFailureReason::DeclaredFailure,
                },
            };
            request
                .snapshot
                .executions
                .insert(node.reference.execution, node);
        }
        let impact = impact::ImpactEvidence::new(&request, Vec::new());
        let episode = impact.episodes.get("worker");
        let references = vec!["v2:1".to_owned(), "v2:2".to_owned()];
        for role in [plan::Role::Graph, plan::Role::Repository] {
            assert_eq!(
                episode.is_some_and(|episode| episode.allows(role, &references)),
                eligible
            );
        }
    }
}

#[tokio::test]
async fn auxiliary_postmortem_output_and_tool_observations_are_not_discovery_evidence() {
    let fixture = self::fixture(Default::default(), None, true).await;
    fixture.drive().await;
    let request = fixture.postmortem_request().await;
    let observations = vec![ToolObservation {
        seq: 1,
        node_id: "active_memory_repository".to_owned(),
        execution_id: AUXILIARY_PROVIDER_SLOT_OFFSET + 1,
        timestamp_ms: 1,
        tool_name: "read_file".to_owned(),
        paths: Vec::new(),
    }];
    let evidence = evidence::collect(&request, observations)
        .await
        .assert_value();
    assert!(evidence.impact.tools.is_empty());
    assert!(!evidence.references.contains("hook:1"));
    let records = evidence.document["records"].as_array().assert_value();
    assert!(!records.is_empty());
    assert!(records.iter().all(|record| {
        !record["event"]
            .as_str()
            .assert_value()
            .contains("active_memory_")
    }));
}
