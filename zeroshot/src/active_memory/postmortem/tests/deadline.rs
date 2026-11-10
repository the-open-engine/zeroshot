use super::*;
use std::time::Duration;

fn spawn(fixture: &Arc<Fixture>) -> tokio::task::JoinHandle<()> {
    let owned = fixture.clone();
    tokio::spawn(async move {
        owned.drive().await;
    })
}

async fn completed(fixture: &Fixture, role: &str, count: usize) {
    let expected = format!("Active Memory active_memory_{role} completed");
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let page = fixture
                .ledger
                .snapshot_and_tail(&fixture.run_id, None)
                .await
                .assert_value();
            let observed = page
                .events
                .iter()
                .filter(|event| {
                    matches!(&event.event,
                RunEvent::SafeLog {line,..} if line.as_str()==expected)
                })
                .count();
            if observed >= count {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .assert_value();
}

async fn held_drive(
    fixture: &Arc<Fixture>,
    role: &str,
    count: usize,
) -> tokio::task::JoinHandle<()> {
    let drive = spawn(fixture);
    fixture.driver.held.notified().await;
    completed(fixture, role, count).await;
    drive
}

async fn graph_discovery_fixture() -> Arc<Fixture> {
    let fixture = Arc::new(fixture(Default::default(), None, true).await);
    fixture.substantial_exploration();
    fixture.output("graph", json!({"lessons":[graph_candidate()]}));
    fixture
}

async fn expire() {
    // Provider startup/source inspection completes in real time before this deterministic cutoff.
    tokio::time::pause();
    tokio::time::advance(Duration::from_millis(600_001)).await;
    tokio::time::resume();
}

fn hold(fixture: &Fixture, name: &'static str, from_batch: u64, from_attempt: u64) {
    *fixture.driver.hold.lock().assert_value() = Some(HeldRole {
        name,
        from_batch,
        from_attempt,
    });
}

fn graph_candidate() -> Value {
    let mut proposal = candidate("worker");
    proposal["repository_specific"] = json!(false);
    for key in ["files", "folders", "identifiers", "keyword_groups"] {
        proposal[key] = json!([]);
    }
    proposal
}

async fn graph_lessons(count: usize) -> Vec<LessonDraft> {
    let template = fixture(Default::default(), None, true).await;
    (0..count)
        .map(|index| {
            let mut draft = seed_lesson();
            draft.id = format!("graph-{index:04}");
            draft.scope = LessonScope::Graph {
                graph_hash: template.query.graph_hash.clone().assert_value(),
                node_id: "worker".to_owned(),
                repository: None,
            };
            draft
        })
        .collect()
}

async fn consolidation(seeds: Vec<LessonDraft>) -> Arc<Fixture> {
    let fixture = Arc::new(fixture_with_lessons(Default::default(), seeds, true).await);
    *fixture.driver.maintenance_action.lock().assert_value() = Some("consolidate");
    fixture
}

async fn assert_timeout(fixture: &Fixture) {
    let events = fixture
        .ledger
        .snapshot_and_tail(&fixture.run_id, None)
        .await
        .assert_value()
        .events;
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, RunEvent::SafeLog {line,..}
        if line.as_str().contains("cancelled=false, timed_out=true")))
    );
}

#[tokio::test]
async fn deadline_publishes_a_complete_specialist_and_rejects_another_roles_late_output() {
    let fixture = graph_discovery_fixture().await;
    fixture.output("repository", json!({"lessons":[candidate("worker")]}));
    hold(&fixture, "active_memory_repository", 1, 0);
    let drive = held_drive(&fixture, "graph", 1).await;
    expire().await;
    drive.await.assert_value();
    let lessons = fixture.lessons().await;
    assert_eq!(lessons.len(), 1);
    assert!(matches!(
        &lessons[0].scope,
        LessonScope::Graph {
            repository: None,
            ..
        }
    ));
    assert_timeout(&fixture).await;
}

#[tokio::test]
async fn a_complete_canonical_upsert_and_duplicate_retirement_survive_an_independent_timeout() {
    let fixture = consolidation(graph_lessons(2).await).await;
    hold(&fixture, "active_memory_repository", 1, 0);
    let drive = held_drive(&fixture, "graph_maintenance", 1).await;
    expire().await;
    drive.await.assert_value();
    let lessons = fixture.lessons().await;
    assert_eq!(lessons.len(), 1);
    assert_eq!(lessons[0].id, "graph-0000");
    assert_eq!(lessons[0].revision, 2);
    assert_timeout(&fixture).await;
}

#[tokio::test]
async fn an_unfinished_later_batch_discards_every_mutation_from_its_specialist() {
    let fixture =
        Arc::new(fixture_with_lessons(Default::default(), graph_lessons(600).await, true).await);
    *fixture.driver.maintenance_action.lock().assert_value() = Some("consolidate");
    hold(&fixture, "active_memory_graph_maintenance", 2, 0);
    let drive = held_drive(&fixture, "graph", 1).await;
    expire().await;
    drive.await.assert_value();
    let retained = fixture.lessons().await;
    assert_eq!(retained.len(), 600);
    assert!(retained.iter().all(|lesson| lesson.revision == 1));
    assert_timeout(&fixture).await;
}

#[tokio::test]
async fn explicit_stop_fences_completed_but_unpublished_specialists() {
    let fixture = graph_discovery_fixture().await;
    hold(&fixture, "active_memory_repository", 1, 0);
    let drive = held_drive(&fixture, "graph", 1).await;
    fixture.supervisor.force_stop().await.assert_value();
    drive.await.assert_value();
    assert!(fixture.lessons().await.is_empty());
}

#[tokio::test]
async fn a_conflict_retry_keeps_a_complete_scope_when_the_other_scope_expires() {
    let mut seeds = graph_lessons(1).await;
    seeds.push(seed_lesson());
    let fixture = Arc::new(fixture_with_lessons(Default::default(), seeds, true).await);
    *fixture.driver.maintenance_action.lock().assert_value() = Some("adapt");
    fixture.concurrent_update();
    hold(&fixture, "active_memory_graph_maintenance", 1, 1);
    let drive = held_drive(&fixture, "repository_maintenance", 2).await;
    expire().await;
    drive.await.assert_value();
    let lessons = fixture.lessons().await;
    assert_eq!(
        lessons
            .iter()
            .find(|lesson| lesson.id == "graph-0000")
            .assert_value()
            .revision,
        1
    );
    assert_eq!(
        lessons
            .iter()
            .find(|lesson| lesson.id == "existing-provider-lesson")
            .assert_value()
            .revision,
        3
    );
    assert_timeout(&fixture).await;
}

#[tokio::test]
async fn unknown_source_change_authority_is_visible_and_does_not_invalidate_a_card() {
    let fixture = fixture(Default::default(), Some(seed_lesson()), true).await;
    let mut legacy = fixture.lessons().await[0].draft();
    legacy
        .evidence
        .references
        .retain(|reference| !reference.starts_with("source:"));
    fixture
        .store
        .lock()
        .await
        .apply_batch(&[LessonMutation::Upsert {
            expected_revision: Some(1),
            draft: Box::new(legacy),
        }])
        .assert_value();
    fixture.output(
        "repository_maintenance",
        json!({"decisions":[decision("adapt")]}),
    );
    fixture.drive().await;
    assert_eq!(fixture.lessons().await[0].revision, 2);
    fixture.assert_source_change_allowed(false);
}

#[tokio::test]
async fn source_drift_discards_the_whole_mixed_scope_consolidation() {
    let mut seeds = graph_lessons(2).await;
    if let LessonScope::Graph { repository, .. } = &mut seeds[0].scope {
        *repository = Some("example/repo".to_owned());
    }
    let fixture = consolidation(seeds).await;
    fixture.substantial_exploration();
    fixture.output("graph", json!({"lessons":[graph_candidate()]}));
    hold(&fixture, "active_memory_repository", 1, 0);
    let drive = held_drive(&fixture, "graph_maintenance", 1).await;
    completed(&fixture, "graph", 1).await;
    std::fs::write(
        fixture._directory.path().join("workspace/src/provider.rs"),
        "// changed after specialist completion\n",
    )
    .assert_value();
    expire().await;
    drive.await.assert_value();
    let lessons = fixture.lessons().await;
    assert_eq!(lessons.len(), 3);
    assert!(lessons.iter().all(|lesson| lesson.revision == 1));
    assert_eq!(
        lessons
            .iter()
            .filter(|lesson| lesson.id.starts_with("graph-"))
            .count(),
        2
    );
    assert!(
        lessons
            .iter()
            .any(|lesson| lesson.id.starts_with("lesson-"))
    );
    assert_timeout(&fixture).await;
}
