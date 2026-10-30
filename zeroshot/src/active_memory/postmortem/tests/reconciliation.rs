use super::*;

#[tokio::test]
async fn maintenance_batches_cover_all_supplied_lessons_and_reject_an_incomplete_last_batch() {
    for omitted in [None, Some("supplied-0599")] {
        let lessons = (0..600)
            .map(|index| {
                let mut draft = seed_lesson();
                draft.id = format!("supplied-{index:04}");
                draft
            })
            .collect();
        let fixture = fixture_with_lessons(Default::default(), lessons, true).await;
        *fixture.driver.maintenance_action.lock().assert_value() = Some("retire");
        *fixture.driver.maintenance_omit.lock().assert_value() = omitted.map(str::to_owned);
        fixture.drive().await;
        let retained = fixture.lessons().await;
        assert_eq!(fixture.maintenance_rejected().await, omitted.is_some());
        if omitted.is_some() {
            assert_eq!(
                retained.len(),
                600,
                "a failed later batch must not publish earlier retirements"
            );
            assert!(retained.iter().all(|lesson| lesson.revision == 1));
        } else {
            assert!(retained.is_empty(), "every supplied lesson must be retired");
        }
        let all_inputs = fixture.driver.maintenance_inputs.lock().assert_value();
        let inputs = all_inputs
            .iter()
            .filter(|input| input["role"] == "active_memory_repository_maintenance")
            .collect::<Vec<_>>();
        assert!(
            inputs.len() > 1,
            "the manifest must exceed one context budget"
        );
        let mut seen = std::collections::BTreeSet::new();
        for (index, input) in inputs.iter().enumerate() {
            assert!(context_budget::encoded_size(input) <= context_budget::MAX_INPUT_BYTES);
            assert_eq!(
                input["maintenance_batch"],
                json!({"index":index + 1,"total":inputs.len()})
            );
            assert_eq!(input["supplied_manifest_total"], 600);
            for id in input["supplied_lessons"].as_object().assert_value().keys() {
                assert!(
                    seen.insert(id.clone()),
                    "a lesson was reviewed more than once"
                );
            }
        }
        assert_eq!(seen.len(), 600);
        assert!(seen.contains("supplied-0599"));
        assert_eq!(
            fixture.driver.globals.load(Ordering::SeqCst),
            inputs.len() + 3
        );
        assert_eq!(
            fixture.driver.closed.load(Ordering::SeqCst),
            5,
            "maintenance batches reuse their specialist session"
        );
    }
}

#[tokio::test]
async fn omitted_duplicate_or_foreign_maintenance_decisions_cannot_claim_completion() {
    let retire = decision("retire");
    let mut foreign = retire.clone();
    foreign["id"] = json!("not-supplied-in-this-batch");
    for decisions in [
        vec![],
        vec![retire.clone(), retire.clone()],
        vec![retire.clone(), foreign],
    ] {
        let fixture = fixture(Default::default(), Some(seed_lesson()), true).await;
        fixture.output("repository_maintenance", json!({"decisions":decisions}));
        fixture.drive().await;
        assert!(fixture.maintenance_rejected().await);
        let lessons = fixture.lessons().await;
        assert_eq!(lessons.len(), 1);
        assert_eq!(
            lessons[0].revision, 1,
            "failed reconciliation must not publish a partial result"
        );
    }
}

#[tokio::test]
async fn discovery_publishes_all_qualifying_findings_without_a_count_quota() {
    let fixture = fixture(Default::default(), None, true).await;
    fixture.substantial_exploration();
    let lessons = (0..129).map(|index| {
        let mut finding = candidate("worker");
        finding["title"] = json!(format!("Provider control interaction {index}"));
        finding["recommendation"] = json!(format!("For provider control interaction {index}, reject cached OAuth credentials before constructing the Bedrock environment."));
        finding
    }).collect::<Vec<_>>();
    fixture.output("repository", json!({"lessons":lessons}));
    fixture.drive().await;
    let stored = fixture.lessons().await;
    assert_eq!(stored.len(), 129);
    assert!(
        stored
            .iter()
            .any(|lesson| lesson.title == "Provider control interaction 128")
    );
}

#[tokio::test]
async fn retry_retains_original_advice_when_initial_maintenance_kept_without_a_mutation() {
    let fixture = fixture(Default::default(), Some(seed_lesson()), true).await;
    *fixture.driver.concurrent_store.lock().assert_value() = Some(fixture.store.clone());
    let mut keep = decision("keep");
    for key in ["title", "recommendation"] {
        keep[key] = json!("");
    }
    for key in ["files", "folders", "identifiers", "keyword_groups"] {
        keep[key] = json!([]);
    }
    fixture.output("repository_maintenance", json!({"decisions":[keep]}));
    fixture.drive().await;
    let inputs = fixture.driver.maintenance_inputs.lock().assert_value();
    let retry = inputs
        .iter()
        .find(|input| {
            input["publication_retry"] == true
                && input["role"] == "active_memory_repository_maintenance"
        })
        .assert_value();
    let change = &retry["lesson_changes"]["existing-provider-lesson"];
    assert_eq!(
        change["before"]["recommendation"],
        seed_lesson().recommendation
    );
    assert_eq!(change["before"]["revision"], 1);
    assert_eq!(change["current"]["revision"], 2);
    assert_eq!(change["worker_received"], true);
    assert!(change["initial_proposal"].is_null());
    assert!(change["previous_proposal"].is_null());
}

#[tokio::test]
async fn retry_only_publishes_verified_kept_or_adapted_findings() {
    for (action, published) in [
        ("defer", false),
        ("retire", false),
        ("keep", true),
        ("adapt", true),
    ] {
        let fixture = fixture(Default::default(), Some(seed_lesson()), true).await;
        fixture.substantial_exploration();
        *fixture.driver.concurrent_store.lock().assert_value() = Some(fixture.store.clone());
        *fixture.driver.maintenance_action.lock().assert_value() = Some(action);
        fixture.output("repository", json!({"lessons":[candidate("worker")]}));
        fixture.drive().await;
        {
            let inputs = fixture.driver.maintenance_inputs.lock().assert_value();
            assert!(
                inputs
                    .iter()
                    .any(|input| input["publication_retry"] == true)
            );
        }
        let discovered = fixture
            .lessons()
            .await
            .into_iter()
            .any(|lesson| lesson.id != "existing-provider-lesson");
        assert_eq!(discovered, published, "action={action}");
        assert!(!fixture.maintenance_rejected().await);
    }
}

#[tokio::test]
async fn rediscovery_of_a_retired_id_stops_without_repeating_unchanged_model_work() {
    let fixture = fixture(Default::default(), None, true).await;
    fixture.substantial_exploration();
    let finding = candidate("worker");
    let scope = LessonScope::Repository {
        repository: fixture.query.repository.clone().assert_value(),
        files: vec!["src/provider.rs".to_owned()],
        folders: vec![],
        identifiers: vec!["reject_provider_controls".to_owned()],
        keyword_groups: vec![vec!["bedrock".to_owned(), "oauth".to_owned()]],
    };
    use sha2::{Digest, Sha256};
    let normalized = finding["recommendation"]
        .as_str()
        .assert_value()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let digest = Sha256::digest(serde_json::to_vec(&(scope.clone(), normalized)).assert_value());
    let mut retired = seed_lesson();
    retired.id = format!("lesson-{digest:x}");
    retired.scope = scope;
    let mut store = fixture.store.lock().await;
    store
        .apply_batch(&[LessonMutation::Upsert {
            expected_revision: None,
            draft: Box::new(retired.clone()),
        }])
        .assert_value();
    store
        .apply_batch(&[LessonMutation::Delete {
            id: retired.id,
            expected_revision: 1,
        }])
        .assert_value();
    drop(store);
    fixture.output("repository", json!({"lessons":[finding]}));
    fixture.drive().await;
    assert!(fixture.lessons().await.is_empty());
    assert_eq!(fixture.driver.globals.load(Ordering::SeqCst), 4);
    let page = fixture
        .ledger
        .snapshot_and_tail(&fixture.run_id, None)
        .await
        .assert_value();
    assert!(page.events.iter().any(|event| matches!(&event.event,
        RunEvent::SafeLog {line,..} if line.as_str().contains("publication conflicted"))));
}

#[tokio::test]
async fn cancelled_queued_publication_does_not_wait_for_or_write_to_the_store() {
    let fixture = fixture(Default::default(), None, true).await;
    let guard = fixture.store.lock().await;
    let backend = SqliteMemoryBackend(fixture.store.clone());
    let publication = MemoryPublication {
        scope: fixture.query.clone(),
        reviewed: vec![],
        mutations: vec![LessonMutation::Upsert {
            draft: Box::new(seed_lesson()),
            expected_revision: None,
        }],
    };
    let (cancel, cancellation) = watch::channel(false);
    let work = backend.publish(&fixture.run_id, publication, cancellation);
    assert!(matches!(
        cancel_pending(work, cancel).await,
        Err(ActiveMemoryError::Cancelled)
    ));
    assert!(
        guard
            .snapshot(&fixture.query)
            .assert_value()
            .lessons
            .is_empty()
    );
}

struct StalledSnapshot;
#[async_trait]
impl ActiveMemoryBackend for StalledSnapshot {
    async fn snapshot(
        &self,
        _: &RunId,
        _: &ScopeQuery,
    ) -> Result<MemorySnapshot, ActiveMemoryError> {
        std::future::pending().await
    }
    async fn publish(
        &self,
        _: &RunId,
        _: MemoryPublication,
        _: watch::Receiver<bool>,
    ) -> Result<Vec<crate::active_memory::LessonVersion>, ActiveMemoryError> {
        panic!("a cancelled snapshot must never publish")
    }
}

#[tokio::test]
async fn cancellation_interrupts_a_stalled_storage_refresh() {
    let fixture = fixture(Default::default(), None, true).await;
    fixture.drive().await;
    let request = fixture.postmortem_request().await;
    let postmortem = BuiltinPostmortem::new(BuiltinPostmortemConfig {
        store: Arc::new(StalledSnapshot),
        factory: fixture.factory.clone(),
        snapshot: MemorySnapshot::default(),
        supplied: fixture.source.clone(),
        repository: fixture.query.repository.clone(),
        graph_hash: fixture.query.graph_hash.clone().assert_value(),
        workspace: fixture._directory.path().join("workspace"),
    });
    let (cancel, cancellation) = watch::channel(false);
    let work = postmortem.current(&request, &fixture.query, cancellation);
    assert!(matches!(
        cancel_pending(work, cancel).await,
        Err(PostmortemError::Cancelled)
    ));
}

async fn cancel_pending<F: std::future::Future>(work: F, cancel: watch::Sender<bool>) -> F::Output {
    tokio::pin!(work);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), &mut work)
            .await
            .is_err()
    );
    cancel.send_replace(true);
    tokio::time::timeout(std::time::Duration::from_secs(1), work)
        .await
        .assert_value()
}
