//! Abrupt runtime loss while the real delivery driver waits for CI, without final settlement.
use super::*;
use crate::native_v2_supervisor::checkpoints::{DeliveryCheckpointStore, RunCheckpointStore};
use crate::v2_run_ledger::sqlite::SqliteRunLedger;

pub(crate) trait ConversationFixture: Send + Sync {
    fn authority(&self) -> Arc<dyn SourceDeliveryAuthority>;
    fn credential(&self) -> SourceDeliveryCredential<'static>;
    fn bind(&self, review: &SourceReviewReceipt);
    fn resolved(&self) -> bool;
    fn edit(&self);
}

struct DriverRuntime {
    kill: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl DriverRuntime {
    fn stop(&mut self) -> std::thread::Result<()> {
        if let Some(kill) = self.kill.take() {
            let _ = kill.send(());
        }
        self.thread.take().map_or(Ok(()), |thread| thread.join())
    }
}
impl Drop for DriverRuntime {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct PendingCiAuthority {
    fixture: Arc<dyn ConversationFixture>,
    forge: Arc<dyn SourceDeliveryAuthority>,
    publication: FakeGitHub,
    polling: AtomicBool,
    review: std::sync::Mutex<Option<SourceReviewReceipt>>,
}

#[async_trait]
impl SourceDeliveryAuthority for PendingCiAuthority {
    fn forge_credential_field(&self) -> Option<&'static str> {
        Some("GH_TOKEN")
    }
    async fn review_conversation_state(&self) -> Result<Option<Value>, SourceDeliveryError> {
        self.forge.review_conversation_state().await
    }
    async fn restore_review_conversation_state(
        &self,
        state: Value,
    ) -> Result<(), SourceDeliveryError> {
        self.forge.restore_review_conversation_state(state).await
    }
    async fn observe_delivery(
        &self,
        request: SourceDeliveryRead<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceDeliverySnapshot, SourceDeliveryError> {
        self.publication.observe_delivery(request, credential).await
    }
    async fn reconcile_delivery_target(
        &self,
        request: SourceTargetReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceTargetIntegration, SourceDeliveryError> {
        self.publication
            .reconcile_delivery_target(request, credential)
            .await
    }
    async fn reconcile_delivery_head(
        &self,
        request: SourceHeadReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
        self.publication
            .reconcile_delivery_head(request, credential)
            .await
    }
    async fn push_branch(
        &self,
        request: &SourcePushRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        self.publication.push_branch(request, credential).await
    }
    async fn open_or_update_review(
        &self,
        request: &SourceReviewRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewReceipt, SourceDeliveryError> {
        let review = self
            .publication
            .open_or_update_review(request, credential)
            .await?;
        self.fixture.bind(&review);
        *self.review.lock().unwrap() = Some(review.clone());
        Ok(review)
    }
    async fn inspect_review(
        &self,
        review: &SourceReviewReceipt,
        _: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewObservation, SourceDeliveryError> {
        if self.fixture.resolved() {
            self.polling.store(true, Ordering::SeqCst);
        }
        Ok(review.observation_with_readiness(
            SourceReviewState::Open {
                checks: SourceChecks::Pending,
            },
            false,
            false,
        ))
    }
    async fn inspect_review_feedback(
        &self,
        review: &SourceReviewReceipt,
        _: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewFeedback, SourceDeliveryError> {
        self.forge
            .inspect_review_feedback(review, self.fixture.credential())
            .await
    }
    async fn resolve_review_conversation(
        &self,
        request: SourceReviewResolutionRequest<'_>,
        _: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewResolution, SourceDeliveryError> {
        self.forge
            .resolve_review_conversation(request, self.fixture.credential())
            .await
    }
    async fn request_merge(
        &self,
        _: &SourceReviewReceipt,
        _: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
        panic!("delivery must still be waiting for CI")
    }
}

pub(crate) async fn assert_interrupted_conversation_driver(fixture: Arc<dyn ConversationFixture>) {
    let repo = TempRepo::delivery();
    let initial = SourceReviewReceipt {
        repository: "acme/project".into(),
        review_id: "17".into(),
        target_branch: "main".into(),
        head_branch: "pending".into(),
        head_revision: repo.base.clone(),
    };
    fixture.bind(&initial);
    let forge = fixture.authority();
    let feedback = forge
        .inspect_review_feedback(&initial, fixture.credential())
        .await
        .unwrap();
    let thread = &feedback.conversations[0];
    let response = SourceReviewResponse {
        thread_id: thread.id.clone(),
        version: thread.version.clone(),
        disposition: SourceReviewDisposition::Addressed,
        response: "Guarded empty input; regression verified.".into(),
    };
    let authority = Arc::new(PendingCiAuthority {
        fixture: fixture.clone(),
        forge,
        publication: FakeGitHub::new(repo.remote.clone(), Script::NoCi),
        polling: AtomicBool::new(false),
        review: Default::default(),
    });
    let schema = json!({"kind":"record","fields":{
        "title":{"type":{"kind":"string"},"required":true},
        "description":{"type":{"kind":"string"},"required":true},
        "reviewResponses":{"type":{"kind":"array","items":{"kind":"record","fields":{
            "threadId":{"type":{"kind":"string"},"required":true},"version":{"type":{"kind":"string"},"required":true},
            "disposition":{"type":{"kind":"enum","values":["addressed","declined"]},"required":true},"response":{"type":{"kind":"string"},"required":true}
        }}},"required":true}
    }});
    let mut node = delivery_node(DeliveryMode::MergeV3);
    node["input"] = schema.clone();
    node["inputBindings"] = json!([{"target":["title"],"value":{"source":"state","path":["title"]}},{"target":["description"],"value":{"source":"state","path":["description"]}},{"target":["reviewResponses"],"value":{"source":"state","path":["reviewResponses"]}}]);
    let mut graph = serde_json::to_value(full_graph(vec![node, success_node()])).unwrap();
    graph["initialInput"] = schema.clone();
    graph["root"]["state"] = schema;
    let binding = NodeRuntimeBinding::GitDelivery {
        connections: DeclaredConnections::single(
            "github",
            DeclaredEnvironment::new([EnvironmentVariableName::new("GH_TOKEN").unwrap()]).unwrap(),
        )
        .unwrap(),
        pull_request_feedback: PullRequestFeedback::Consider,
    };
    let mut runtime = delivery_runtime(binding);
    runtime.workspace_checkpoints = false;
    let admitted = NativeV2Admission.admit(RunSubmission { environment: None, title: RunTitle::new("Interrupted conversation delivery").unwrap(), graph: serde_json::from_value(graph).unwrap(), initial_input: json!({"title":"fix: guard input","description":"Guard empty input.","reviewResponses":[response]}), runtime, source: source_for_revision(&repo.base), submission_key: IdempotencyKey::new("interrupted-conversation").unwrap() }).await.unwrap();
    let delivery = retained_adapter(
        &repo,
        authority.clone(),
        DeliveryPollPolicy::new(usize::MAX, Duration::from_millis(10)).unwrap(),
        DeliveryLineage::original("interrupted-conversation"),
    );
    let runner = Arc::new(NativeNodeRunner::new(&admitted, delivery.clone(), delivery).unwrap());
    let environment = Arc::new(
        RunEnvironment::exact(
            &admitted.runtime,
            None,
            BTreeMap::from([(
                ConnectionKey::new("github").unwrap(),
                StaticConnectionValues::new(BTreeMap::from([(
                    EnvironmentVariableName::new("GH_TOKEN").unwrap(),
                    "test-token".into(),
                )]))
                .unwrap(),
            )]),
        )
        .unwrap(),
    );
    let path = repo.root.path().join("run.sqlite3");
    let ledger = Arc::new(SqliteRunLedger::open(&path).unwrap());
    let run_id = RunId::new("interrupted-conversation");
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("interrupted-conversation").unwrap(),
            submission_digest: Sha256Digest::new("d".repeat(64)).unwrap(),
            admitted,
        })
        .await
        .unwrap();
    let (kill, lost) = tokio::sync::oneshot::channel();
    let supervisor = NativeV2Supervisor::new(run_id.clone(), ledger.clone(), runner, environment);
    let thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::select! {
                result = supervisor.drive() => panic!("delivery must remain active: {result:?}"),
                _ = lost => {},
            }
        });
        // Drop all tasks without giving delivery its final state-export/cleanup turn.
        runtime.shutdown_timeout(Duration::ZERO);
    });
    let mut runtime = DriverRuntime {
        kill: Some(kill),
        thread: Some(thread),
    };
    tokio::time::timeout(Duration::from_secs(15), async {
        while !authority.polling.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the real delivery driver must resolve then poll pending CI");
    assert!(fixture.resolved());
    let active = ledger.get(&run_id).await.unwrap().unwrap();
    assert!(active.snapshot.terminal.is_none());
    assert!(
        active
            .snapshot
            .executions
            .values()
            .all(|node| node.outcome().is_none())
    );
    let acknowledged = active
        .snapshot
        .delivery_checkpoint()
        .unwrap()
        .expect("receipt is durable before delivery completes");
    assert!(
        acknowledged.generation.get() >= 2,
        "pending and confirmed effects are separately acknowledged"
    );
    tokio::task::spawn_blocking(move || runtime.stop().unwrap())
        .await
        .unwrap();
    let reopened = SqliteRunLedger::open(&path)
        .unwrap()
        .get(&run_id)
        .await
        .unwrap()
        .unwrap();
    let recovered = reopened.snapshot.delivery_checkpoint().unwrap().unwrap();
    assert_eq!(recovered, acknowledged);
    let lineage = DeliveryCheckpointStore::new(repo.root.path().join("lineage"));
    lineage.retain_delivery_state(&recovered).await.unwrap();
    let fresh = fixture.authority();
    fresh
        .restore_review_conversation_state(lineage.delivery_state().await.unwrap().unwrap().state)
        .await
        .unwrap();
    fixture.edit();
    let review = authority.review.lock().unwrap().clone().unwrap();
    // A later worker returns []; recovery must depend on the acknowledged receipt alone.
    assert_eq!(
        fresh
            .request_merge(&review, fixture.credential())
            .await
            .unwrap(),
        SourceMergeRequestOutcome::ReviewBlocked
    );
    let feedback = fresh
        .inspect_review_feedback(&review, fixture.credential())
        .await
        .unwrap();
    assert_eq!(feedback.items.len(), 1);
    assert_eq!(feedback.conversations.len(), 1);
    assert!(!fixture.resolved());
}
