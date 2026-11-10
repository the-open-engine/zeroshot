use super::*;
use crate::active_memory::{ActiveMemoryStore, LessonDraft};
use std::time::Duration;

struct Fixture {
    _directory: tempfile::TempDir,
    store: SharedActiveMemoryStore,
    scope: ScopeQuery,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store =
            ActiveMemoryStore::open(&directory.path().join("state/lessons.sqlite")).unwrap();
        Self {
            _directory: directory,
            store: Arc::new(tokio::sync::Mutex::new(store)),
            scope: ScopeQuery {
                repository: Some("example/repo".into()),
                repository_lessons: true,
                ..Default::default()
            },
        }
    }

    fn publication(&self) -> MemoryPublication {
        let draft: LessonDraft = serde_json::from_value(serde_json::json!({
            "id":"publication-gate","scope":{"kind":"repository","repository":"example/repo",
                "files":["source.rs"],"folders":[],"identifiers":[],"keyword_groups":[]},
            "title":"Preserve the publication boundary",
            "recommendation":"Check the original stop signal immediately before dispatching a memory write.",
            "evidence":{"run_id":"fixture-run","node_id":"worker","references":["v2:1"],
                "costly_recurring_reason":"Queued writes must not outlive the original cancellation authority."}
        })).unwrap();
        MemoryPublication {
            scope: self.scope.clone(),
            reviewed: vec![],
            mutations: vec![LessonMutation::Upsert {
                expected_revision: None,
                draft: Box::new(draft),
            }],
        }
    }
}

#[test]
fn original_hard_stop_is_visible_when_a_queued_blocking_write_reaches_dispatch() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let fixture = Fixture::new();
        let (release, waiting) = std::sync::mpsc::channel();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            entered.send(()).unwrap();
            let _ = waiting.recv();
        });
        ready.await.unwrap();
        let backend = SqliteMemoryBackend(fixture.store.clone());
        let publication = fixture.publication();
        let (original_stop, control) = publication_control();
        let publication = tokio::spawn(async move {
            backend
                .publish(&RunId::new("fixture-run"), publication, control)
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while fixture.store.try_lock().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // The only blocking worker is held: the backend owns the store but cannot pass its gate yet.
        original_stop.send_replace(true);
        release.send(()).unwrap();
        assert!(matches!(
            publication.await.unwrap(),
            Err(ActiveMemoryError::Cancelled)
        ));
        blocker.await.unwrap();
        assert!(
            fixture
                .store
                .lock()
                .await
                .snapshot(&fixture.scope)
                .unwrap()
                .lessons
                .is_empty()
        );
    });
}

#[tokio::test(start_paused = true)]
async fn a_queued_write_observes_its_absolute_deadline_without_releasing_the_store() {
    let fixture = Fixture::new();
    let held = fixture.store.lock().await;
    let backend = SqliteMemoryBackend(fixture.store.clone());
    let (_original_stop, original_signal) = watch::channel(false);
    let control =
        MemoryPublicationControl::new(original_signal, Instant::now() + Duration::from_millis(10));
    let result = backend
        .publish(&RunId::new("fixture-run"), fixture.publication(), control)
        .await;
    assert!(matches!(result, Err(ActiveMemoryError::Cancelled)));
    assert!(held.snapshot(&fixture.scope).unwrap().lessons.is_empty());
}

#[test]
fn a_write_that_passed_the_dispatch_gate_drains_after_the_original_stop() {
    let fixture = Fixture::new();
    let mut store = fixture.store.try_lock().unwrap();
    let publication = fixture.publication();
    let (original_stop, control) = publication_control();
    let result = dispatch_write(&control, || {
        // The gate has accepted this operation: a later stop cannot discard its result.
        original_stop.send_replace(true);
        assert!(control.is_cancelled());
        store.apply_batch(&publication.mutations)
    })
    .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(store.snapshot(&fixture.scope).unwrap().lessons.len(), 1);
}

#[tokio::test]
async fn closing_the_original_stop_channel_revokes_queued_write_authority() {
    let fixture = Fixture::new();
    let backend = SqliteMemoryBackend(fixture.store.clone());
    let (original_stop, control) = publication_control();
    drop(original_stop);
    assert!(control.is_cancelled());
    assert!(matches!(
        backend
            .publish(&RunId::new("fixture-run"), fixture.publication(), control)
            .await,
        Err(ActiveMemoryError::Cancelled)
    ));
    assert!(
        fixture
            .store
            .lock()
            .await
            .snapshot(&fixture.scope)
            .unwrap()
            .lessons
            .is_empty()
    );
}

fn publication_control() -> (watch::Sender<bool>, MemoryPublicationControl) {
    let (original_stop, original_signal) = watch::channel(false);
    let control =
        MemoryPublicationControl::new(original_signal, Instant::now() + Duration::from_secs(30));
    (original_stop, control)
}
