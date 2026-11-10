//! Production recall and maintenance against an explicitly selected real provider.
use openengine_cluster_protocol::{Sha256Digest, TerminalResult};

use super::*;
use crate::active_memory::{
    ActiveMemoryStore, LessonDraft, LessonEvidence, LessonMutation, LessonScope, ScopeQuery,
    SqliteMemoryBackend, MemorySnapshot,
};
use crate::active_memory::runtime::{
    MemoryPreparation, PreparedActiveMemory, authored_graph_hash, repository_key,
};
use crate::native_v2_supervisor::{NativeV2Supervisor, RunEnvironment};
use crate::v2_run_ledger::{CreateRun, RunEvent, RunLedger, StoredRunEvent};
use crate::v2_run_ledger::fake::FakeRunLedger;

const PROOF: &str = ".zeroshot-memory-proof.json";

fn seed_lessons(
    scope: &ScopeQuery,
    graph: &str,
    repository: &str,
    source: &str,
) -> Vec<LessonMutation> {
    let repository_scope = LessonScope::Repository {
        repository: scope.repository.clone().assert_value(),
        files: vec!["README.md".into()],
        folders: vec![],
        identifiers: vec![],
        keyword_groups: vec![],
    };
    let mut lessons: Vec<_> = [
        ("live-graph", "Record graph proof", format!("Write the graph proof marker {graph} to the graph field of {PROOF}."),
            LessonScope::Graph { graph_hash: scope.graph_hash.clone().assert_value(), node_id: "worker".into(), repository: scope.repository.clone() }),
        ("live-repository", "Record repository proof", format!("When inspecting README.md, write the repository proof marker {repository} to the repository field of {PROOF}."), repository_scope.clone()),
        ("live-stale-timeout", "Preserve configured timeout", "README.md defines TIMEOUT_MS as 120000; preserve that value when updating this fixture's runtime configuration.".into(), repository_scope),
    ].into_iter().map(|(id, title, recommendation, scope)| LessonMutation::Upsert {
        expected_revision: None,
        draft: Box::new(LessonDraft {
            id: id.into(), title: title.into(), recommendation, scope,
            evidence: LessonEvidence {
                run_id: "live-fixture-origin".into(), node_id: "worker".into(),
                references: vec!["execution:1".into(), format!("source:{source}")],
                costly_recurring_reason: "Controlled live fixture for recall and stale-source reconciliation.".into(),
            },
        }),
     }).collect();
    for (template, prefix, count) in [(0, "graph", 40), (1, "repository", 64)] {
        let LessonMutation::Upsert { draft, .. } = &lessons[template] else {
            panic!("fixture seed must be an upsert")
        };
        let template = draft.as_ref().clone();
        lessons.extend((0..count).map(|index| {
            let mut draft = template.clone();
            draft.id = format!("bulk-{prefix}-{index:03}");
            draft.title = format!("Transport fixture padding {prefix} {index}");
            draft.recommendation = format!(
                "Controlled padding for a large memory injection. {}",
                "reserved context ".repeat(32).trim_end()
            );
            LessonMutation::Upsert {
                expected_revision: None,
                draft: Box::new(draft),
            }
        }));
    }
    lessons
}

#[test]
fn generous_live_fixture_validates_before_spending_provider_calls() {
    let scope = ScopeQuery {
        repository: Some("repo:live-fixture".into()),
        graph_hash: Some("a".repeat(64)),
        repository_lessons: true,
        graph_lessons: true,
        ..Default::default()
    };
    let lessons = seed_lessons(&scope, "graph-nonce", "repository-nonce", &"b".repeat(40))
        .into_iter()
        .map(|mutation| {
            let LessonMutation::Upsert { draft, .. } = mutation else {
                panic!("fixture seeds must be upserts")
            };
            draft.validate().assert_value();
            (*draft).into_lesson(1)
        })
        .collect();
    let index = crate::active_memory::MemoryIndex::new(MemorySnapshot {
        generation: 1,
        lessons,
    })
    .assert_value();
    let graph = index.graph_guidance("worker", 64000, &BTreeSet::new());
    let repository = index.retrieve(
        &crate::active_memory::RetrievalQuery {
            workspace: "/workspace".into(),
            tool_cwd: Some("/workspace".into()),
            paths: vec!["README.md".into()],
            ..Default::default()
        },
        64000,
        &BTreeSet::new(),
    );
    assert_eq!(graph.cards.len(), 41);
    assert_eq!(repository.cards.len(), 66);
    assert!(graph.bytes_used > 16384 && repository.bytes_used > 32768);
    assert!(graph.context.contains("graph-nonce"));
    assert!(repository.context.contains("repository-nonce"));
    println!(
        "live injection bytes: node={}, hook={}",
        graph.bytes_used, repository.bytes_used
    );
}

#[tokio::test]
#[ignore = "requires explicit live provider settings and a clean disposable Git fixture with an older source basis"]
async fn real_provider_uses_production_recall_and_reconciles_current_source() {
    generous_live_fixture_validates_before_spending_provider_calls();
    let settings = LiveSettings::read();
    let graph = format!("graph-proof-{}", uuid::Uuid::now_v7());
    let repository = format!("repository-proof-{}", uuid::Uuid::now_v7());
    let control = run_fixture(&settings, None).await;
    assert_eq!(
        control["graph"],
        Value::Null,
        "a control run cannot know the memory-only nonce"
    );
    assert_eq!(control["repository"], Value::Null);
    let recalled = run_fixture(&settings, Some((graph.clone(), repository.clone()))).await;
    assert_eq!(
        recalled["graph"], graph,
        "graph recall must affect the provider's artifact"
    );
    assert_eq!(
        recalled["repository"], repository,
        "native tool recall must affect the provider's artifact"
    );
    assert_eq!(control["timeoutMs"], 600000);
    assert_eq!(
        recalled["timeoutMs"], 600000,
        "stale recall cannot override current configuration"
    );
}

async fn admit_fixture(
    settings: &LiveSettings,
    registry: &HarnessRegistry,
    enabled: bool,
    key: IdempotencyKey,
) -> crate::native_v2_contract::AdmittedRun {
    let worker = format!(
        "Inspect README.md and package.json with native tools and run npm test. Write {PROOF} as a JSON object with graph and repository fields containing their exact proof markers supplied by Active Memory, or null when no marker was supplied. Include a numeric timeoutMs field with the actual TIMEOUT_MS from README.md. Follow current checked-in configuration if stored advice disagrees. Do not edit tracked source or run Git commands."
    );
    let verifier = format!(
        "Independently read README.md and {PROOF}, then run npm test using native tools. Report both proof fields and the actual TIMEOUT_MS. Do not edit files or run Git commands."
    );
    let mut runtime = serde_json::to_value(RuntimePlan::new(
        settings.lane.clone(),
        run_size("medium"),
        runtime_bindings(settings, registry),
    ))
    .assert_value();
    runtime["activeMemory"] =
        json!({"repository":enabled, "graph":enabled, "hookInjectionTokens":16000});
    runtime["globalAgents"] = json!({"postmortem":{"enabled":enabled,"sourceNode":"worker"}});
    NativeV2Admission
        .admit(RunSubmission {
            title: RunTitle::new("Production memory live control").assert_value(),
            graph: worker_verifier_graph(&worker, &verifier),
            initial_input: Value::Null,
            runtime: serde_json::from_value(runtime).assert_value(),
            environment: None,
            source: None,
            submission_key: key.clone(),
        })
        .await
        .assert_value()
}

async fn run_fixture(settings: &LiveSettings, markers: Option<(String, String)>) -> Value {
    let enabled = markers.is_some();
    let registry = HarnessRegistry::local();
    let directory = TestDirectory::new("production-live-memory");
    let run_id = RunId::new(format!("memory-live-{}", uuid::Uuid::now_v7()));
    let key = IdempotencyKey::new(run_id.as_str()).assert_value();
    let admitted = admit_fixture(settings, &registry, enabled, key.clone()).await;
    let store = Arc::new(tokio::sync::Mutex::new(
        ActiveMemoryStore::open(&directory.child("memory/lessons.sqlite")).assert_value(),
    ));
    let scope = ScopeQuery {
        repository: Some(repository_key(&admitted, &settings.workspace)),
        graph_hash: Some(authored_graph_hash(&admitted).assert_value()),
        repository_lessons: true,
        graph_lessons: true,
        ..Default::default()
    };
    if let Some((graph, repository)) = markers {
        store
            .lock()
            .await
            .apply_batch(&seed_lessons(
                &scope,
                &graph,
                &repository,
                &required("ZEROSHOT_LIVE_SOURCE_BASIS"),
            ))
            .assert_value();
    }
    let memory = PreparedActiveMemory::prepare_hosted(
        MemoryPreparation {
            admitted: &admitted,
            workspace: &settings.workspace,
            storage: None,
        },
        Some(SqliteMemoryBackend::shared(store.clone())),
        &run_id,
    )
    .await
    .assert_value();
    let runner = build_local_native_v2_candidate(
        &admitted,
        NativeV2CandidateConfig {
            active_memory: memory.as_ref().map(|memory| memory.hooks.clone()),
            lanes: vec![
                registry
                    .configure(settings.context(&registry, directory.child("runtime")))
                    .assert_value(),
            ],
            workspace: settings.workspace.clone(),
            delivery: None,
            comment_delivery: None,
        },
    )
    .assert_value();
    let runner: Arc<dyn NodeRunner> = match memory {
        Some(memory) => memory.wrap(&admitted, runner),
        None => Arc::new(runner),
    };
    let ledger = Arc::new(FakeRunLedger::new());
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: key,
            submission_digest: Sha256Digest::new("0".repeat(64)).assert_value(),
            admitted: admitted.clone(),
        })
        .await
        .assert_value();
    let environment = live_environment(settings, &admitted);
    let supervisor =
        NativeV2Supervisor::new(run_id.clone(), ledger.clone(), runner.clone(), environment)
            .with_postmortem(runner.postmortem());
    let terminal = supervisor.drive().await.assert_value();
    runner.close_run(&run_id).await;
    let events = all_events(ledger.as_ref(), &run_id).await;
    let lessons = store.lock().await.snapshot(&scope).assert_value();
    let proof = std::fs::read(settings.workspace.join(PROOF))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .unwrap_or(Value::Null);
    save_evidence(enabled, &events, &lessons, &proof);
    assert!(
        matches!(terminal, TerminalResult::Succeeded { .. }),
        "live primary must succeed: {terminal:?}"
    );
    assert!(
        proof.is_object(),
        "provider must write the proof JSON object"
    );
    if enabled {
        assert_maintenance(&events, &lessons, &proof);
    }
    proof
}

fn assert_maintenance(events: &[StoredRunEvent], lessons: &MemorySnapshot, proof: &Value) {
    let logs = serde_json::to_string(&events).assert_value();
    assert!(
        logs.contains("supplied_revisions=107"),
        "both large injection sets must reach the provider"
    );
    let primary = events
        .iter()
        .filter_map(|event| match &event.event {
            RunEvent::NodeCompleted { completion } => Some(&completion.outcome),
            _ => None,
        })
        .collect::<Vec<_>>();
    let primary = serde_json::to_string(&primary).assert_value();
    for field in ["graph", "repository"] {
        assert!(
            primary.contains(proof[field].as_str().assert_value()),
            "memory must affect the primary graph before maintenance"
        );
    }
    assert!(
        logs.contains("Active Memory postmortem completed"),
        "live maintenance must finish"
    );
    let stale = lessons
        .lessons
        .iter()
        .find(|lesson| lesson.id == "live-stale-timeout");
    assert!(
        stale.is_none_or(|lesson| lesson.revision > 1
            && (lesson.recommendation.contains("600000")
                || !lesson.recommendation.contains("120000"))),
        "maintenance must adapt or retire advice contradicted by current clean source: {stale:?}"
    );
    if let Some(lesson) = stale {
        assert!(lesson.evidence.references.contains(&format!(
            "source:{}",
            required("ZEROSHOT_LIVE_CURRENT_SOURCE")
        )));
    }
}

fn live_environment(
    settings: &LiveSettings,
    admitted: &crate::native_v2_contract::AdmittedRun,
) -> Arc<RunEnvironment> {
    let connections = admitted
        .runtime
        .nodes()
        .values()
        .flat_map(|binding| binding.declared_connections().iter())
        .map(|(key, fields)| {
            let values = fields
                .iter()
                .map(|name| (name.clone(), settings.environment[name.as_str()].clone()))
                .collect();
            (
                key.clone(),
                openengine_cluster_protocol::StaticConnectionValues::new(values).assert_value(),
            )
        })
        .collect();
    Arc::new(RunEnvironment::exact_for_admitted(admitted, connections).assert_value())
}

fn save_evidence(
    enabled: bool,
    events: &[StoredRunEvent],
    lessons: &MemorySnapshot,
    proof: &Value,
) {
    let Some(root) = std::env::var_os("ZEROSHOT_LIVE_EVIDENCE_DIR") else {
        return;
    };
    let root = PathBuf::from(root);
    std::fs::create_dir_all(&root).assert_value();
    let phase = if enabled { "enabled" } else { "control" };
    for (name, value) in [
        ("events", serde_json::to_value(events).assert_value()),
        ("lessons", serde_json::to_value(lessons).assert_value()),
        ("proof", proof.clone()),
    ] {
        std::fs::write(
            root.join(format!("{phase}-{name}.json")),
            serde_json::to_vec_pretty(&value).assert_value(),
        )
        .assert_value();
    }
}

async fn all_events(ledger: &dyn RunLedger, run: &RunId) -> Vec<StoredRunEvent> {
    let mut cursor = None;
    let mut events = Vec::new();
    loop {
        let page = ledger
            .snapshot_and_tail(run, cursor.as_ref())
            .await
            .assert_value();
        let next = page.events.last().map(|event| event.cursor.clone());
        let complete = next.as_ref() == Some(&page.snapshot.cursor);
        assert!(next.is_some(), "live ledger replay must advance");
        events.extend(page.events);
        if complete {
            return events;
        }
        cursor = next;
    }
}
