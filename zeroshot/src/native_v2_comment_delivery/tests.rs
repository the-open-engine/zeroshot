use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

use openengine_cluster_protocol::{NodeName, WorkerRef};
use crate::native_v2_contract::{ExecutionId, ExecutionRef, NodeInstanceId};
use crate::native_v2_runner::{NodeResponseContract, NodeRole};
use crate::source_code_provider::SourceCommentPage;

use super::*;

const URL: &str = "https://github.com/acme/project/issues/17";

#[derive(Default)]
struct Forge {
    comments: Mutex<Vec<SourceComment>>,
    creates: AtomicUsize,
    actors: AtomicUsize,
    lose_response: AtomicBool,
    omit_publication: AtomicBool,
    stale_scope: AtomicBool,
    actor_switch: AtomicBool,
    reject_posts: AtomicBool,
    reject_reads: AtomicBool,
    cancel_after_create: Mutex<Option<tokio::sync::watch::Sender<bool>>>,
}

#[async_trait]
impl SourceCommentAuthority for Forge {
    fn comment_credential_field(&self) -> &'static str {
        "GH_TOKEN"
    }
    async fn actor(
        &self,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<String, SourceDeliveryError> {
        self.actors.fetch_add(1, Ordering::SeqCst);
        Ok(
            if self.actor_switch.load(Ordering::SeqCst) && credential.expose() == "new-token" {
                "DIFFERENT_ACTOR"
            } else {
                "ACTOR_ID"
            }
            .into(),
        )
    }
    async fn list_comments(
        &self,
        _destination: &SourceCommentDestination,
        _page: u32,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceCommentPage, SourceDeliveryError> {
        if self.reject_reads.swap(false, Ordering::SeqCst) {
            return Err(SourceDeliveryError::api(Some(401), "expired"));
        }
        Ok(SourceCommentPage {
            comments: self.comments.lock().unwrap().clone(),
            has_more: false,
        })
    }
    async fn create_comment(
        &self,
        destination: &SourceCommentDestination,
        body: &str,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceComment, SourceDeliveryError> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        if self.reject_posts.swap(false, Ordering::SeqCst) {
            return Err(SourceDeliveryError::api(Some(401), "expired"));
        }
        let comment = SourceComment {
            id: "23".into(),
            url: format!("{}#issuecomment-23", destination.url),
            discussion_url: destination.url.clone(),
            author: "ACTOR_ID".into(),
            body: body.into(),
        };
        if !self.omit_publication.load(Ordering::SeqCst) {
            self.comments.lock().unwrap().push(comment.clone());
        }
        if let Some(cancel) = self.cancel_after_create.lock().unwrap().as_ref() {
            let _ = cancel.send(true);
        }
        if self.lose_response.load(Ordering::SeqCst) {
            return Err(SourceDeliveryError::api(None, "response lost"));
        }
        Ok(comment)
    }
    async fn get_comment(
        &self,
        _destination: &SourceCommentDestination,
        id: &str,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceComment, SourceDeliveryError> {
        if self.reject_reads.swap(false, Ordering::SeqCst) {
            return Err(SourceDeliveryError::api(Some(401), "expired"));
        }
        self.comments
            .lock()
            .unwrap()
            .iter()
            .find(|comment| comment.id == id)
            .cloned()
            .ok_or(SourceDeliveryError::Rejected)
    }
    async fn verify_review_scope(
        &self,
        _destination: &SourceCommentDestination,
        _scope: &Value,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        if self.stale_scope.load(Ordering::SeqCst) {
            Err(SourceDeliveryError::identity("review_scope_changed"))
        } else {
            Ok(())
        }
    }
}

struct RotatedCredential(ResolvedEnvironment);

#[async_trait]
impl crate::native_v2_runner::RuntimeEnvironmentRefresh for RotatedCredential {
    async fn refresh(
        &self,
    ) -> Result<ResolvedEnvironment, crate::native_v2_runner::EnvironmentRefreshError> {
        Ok(self.0.clone())
    }
}

async fn run_with_refresh(
    adapter: &NativeV2CommentDeliveryAdapter,
) -> Result<WorkerOutcome, NodeRunnerError> {
    let mut invocation = invocation(adapter, json!({"url":URL,"body":"report"})).await;
    let binding: NodeRuntimeBinding = serde_json::from_value(
        json!({"kind":"comment_delivery","connections":{"github":["GH_TOKEN"]}}),
    )
    .unwrap();
    let environment = |token: &str| {
        ResolvedEnvironment::exact(
            &binding,
            BTreeMap::from([(
                EnvironmentVariableName::new("GH_TOKEN").unwrap(),
                token.to_owned(),
            )]),
        )
        .unwrap()
    };
    invocation.environment = crate::native_v2_runner::with_environment_refresh(
        environment("old-token"),
        Arc::new(RotatedCredential(environment("new-token"))),
    );
    invocation.node.binding = binding;
    let (_cancel, control, _durable) = crate::native_v2_runner::test_support::control();
    adapter.run(invocation, control).await
}

#[tokio::test]
async fn renewed_post_authority_cannot_change_author_before_another_post() {
    let temp = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::default());
    forge.reject_posts.store(true, Ordering::SeqCst);
    forge.actor_switch.store(true, Ordering::SeqCst);
    let adapter = adapter(temp.path().join("publication.json"), forge.clone());
    let error = run_with_refresh(&adapter).await.unwrap_err();
    assert!(error.to_string().contains("changed the publication author"));
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
    assert!(matches!(
        adapter.store.load().await.unwrap().unwrap().status,
        PublicationStatus::Ready
    ));
}

#[tokio::test]
async fn expired_scan_credential_renews_once_and_retains_author_identity() {
    let temp = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::default());
    forge.reject_reads.store(true, Ordering::SeqCst);
    let adapter = adapter(temp.path().join("publication.json"), forge.clone());
    run_with_refresh(&adapter).await.unwrap();
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
    assert_eq!(forge.actors.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancellation_after_publication_retains_confirmation_for_successor() {
    let temp = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::default());
    let adapter = adapter(temp.path().join("publication.json"), forge.clone());
    let input = json!({"url":URL,"body":"report"});
    let invocation = invocation(&adapter, input.clone()).await;
    let (cancel, control, _durable) = crate::native_v2_runner::test_support::control();
    *forge.cancel_after_create.lock().unwrap() = Some(cancel);
    assert!(matches!(
        adapter.run(invocation, control).await,
        Err(NodeRunnerError::Cancelled)
    ));
    assert!(matches!(
        adapter.store.load().await.unwrap().unwrap().status,
        PublicationStatus::Confirmed { .. }
    ));
    run(&adapter, input).await.unwrap();
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
}

fn adapter(path: std::path::PathBuf, forge: Arc<Forge>) -> NativeV2CommentDeliveryAdapter {
    NativeV2CommentDeliveryAdapter::new(
        NativeV2CommentDeliveryConfig {
            delivery_run_id: RunId::new("root-review"),
            expected_url: URL.into(),
            expected_review_scope: None,
            source_revision: None,
        },
        forge,
        Arc::new(LocalCommentPublicationStore::new(path)),
    )
}

async fn invocation(adapter: &NativeV2CommentDeliveryAdapter, input: Value) -> DriverInvocation {
    let binding = NodeRuntimeBinding::CommentDelivery {
        connections: Default::default(),
    };
    let environment = ResolvedEnvironment::exact(&binding, BTreeMap::new()).unwrap();
    let node = NodeInvocation {
        reference: ExecutionRef {
            run_id: RunId::new("attempt"),
            node: NodeName::new("deliver").unwrap(),
            node_instance: NodeInstanceId::new(1).unwrap(),
            execution: ExecutionId::new(1).unwrap(),
        },
        worker: WorkerRef::new(COMMENT_DELIVERY_WORKER_REF).unwrap(),
        instructions: None,
        input,
        binding,
    };
    let session = adapter.open(&node, &environment).await.unwrap();
    DriverInvocation {
        node,
        role: NodeRole::CommentDelivery,
        response: NodeResponseContract::Verifier {
            output: comment_result_schema().unwrap(),
            signals: comment_signal_labels().unwrap(),
            diagnostic: comment_diagnostic_schema().unwrap(),
        },
        environment,
        session,
        provider_session_slot: NodeInstanceId::new(1).unwrap(),
    }
}

async fn run(
    adapter: &NativeV2CommentDeliveryAdapter,
    input: Value,
) -> Result<WorkerOutcome, NodeRunnerError> {
    let invocation = invocation(adapter, input).await;
    let (_cancel, control, _durable) = crate::native_v2_runner::test_support::control();
    adapter.run(invocation, control).await
}

#[tokio::test]
async fn source_less_comment_confirms_before_receipt_and_survives_store_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("publication.json");
    let forge = Arc::new(Forge::default());
    let first = adapter(path.clone(), forge.clone());
    let input = json!({"url":URL,"body":"Review found two defects."});
    let result = run(&first, input.clone()).await.unwrap();
    let WorkerOutcome::Verifier { output, .. } = &result else {
        panic!("receipt missing");
    };
    assert!(is_matching_success_receipt(output, URL, None));
    assert!(output["receipt"].get("sourceRevision").is_none());
    let second = adapter(path, forge.clone());
    assert_eq!(result, run(&second, input).await.unwrap());
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
    assert!(matches!(
        second.store.load().await.unwrap().unwrap().status,
        PublicationStatus::Confirmed { .. }
    ));
}

#[tokio::test]
async fn lost_post_response_reconciles_exact_author_marker_and_body() {
    let temp = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::default());
    forge.lose_response.store(true, Ordering::SeqCst);
    let adapter = adapter(temp.path().join("publication.json"), forge.clone());
    run(&adapter, json!({"url":URL,"body":"A report"}))
        .await
        .unwrap();
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
    assert!(matches!(
        adapter.store.load().await.unwrap().unwrap().status,
        PublicationStatus::Confirmed { .. }
    ));
}

#[tokio::test]
async fn uncertain_absent_post_is_never_blindly_repeated_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("publication.json");
    let forge = Arc::new(Forge::default());
    forge.lose_response.store(true, Ordering::SeqCst);
    forge.omit_publication.store(true, Ordering::SeqCst);
    let input = json!({"url":URL,"body":"A report"});
    let error = run(&adapter(path.clone(), forge.clone()), input.clone())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("delivery_unconfirmed"));
    let error = run(&adapter(path, forge.clone()), input).await.unwrap_err();
    assert!(error.to_string().contains("prior create"));
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn changed_initial_destination_fails_before_provider_or_publication_storage() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("publication.json");
    let forge = Arc::new(Forge::default());
    let adapter = adapter(path.clone(), forge.clone());
    let error = run(
        &adapter,
        json!({"url":"https://github.com/acme/project/issues/18","body":"redirect"}),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("changed after admission"));
    assert_eq!(forge.actors.load(Ordering::SeqCst), 0);
    assert!(!path.exists());
}

#[tokio::test]
async fn older_checkpoint_body_cannot_replace_a_delivered_report() {
    let temp = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::default());
    let adapter = adapter(temp.path().join("publication.json"), forge.clone());
    run(&adapter, json!({"url":URL,"body":"newer report"}))
        .await
        .unwrap();
    let error = run(
        &adapter,
        json!({"url":URL,"body":"older checkpoint report"}),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("comment_already_delivered"));
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn modified_confirmed_comment_is_not_an_authoritative_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::default());
    let adapter = adapter(temp.path().join("publication.json"), forge.clone());
    let input = json!({"url":URL,"body":"report"});
    run(&adapter, input.clone()).await.unwrap();
    forge.comments.lock().unwrap()[0].body.push_str("tampered");
    assert!(
        run(&adapter, input)
            .await
            .unwrap_err()
            .to_string()
            .contains("did not match")
    );
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stale_scope_refuses_before_durable_create_intent() {
    let temp = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::default());
    forge.stale_scope.store(true, Ordering::SeqCst);
    let mut adapter = adapter(temp.path().join("publication.json"), forge.clone());
    Arc::make_mut(&mut adapter.config).expected_review_scope = Some(json!({}));
    let error = run(
        &adapter,
        json!({"url":URL,"body":"report","reviewScope":{}}),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("review_scope_changed"));
    assert!(adapter.store.load().await.unwrap().is_none());
    assert_eq!(forge.creates.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn atomic_intent_cas_never_grants_create_twice() {
    let temp = tempfile::tempdir().unwrap();
    let store = LocalCommentPublicationStore::new(temp.path().join("publication.json"));
    let record = PublicationRecord {
        version: 1,
        url: URL.into(),
        marker: marker(&RunId::new("root"), "deliver"),
        body_hash: hash("report"),
        author: "ACTOR_ID".into(),
        status: PublicationStatus::Attempted,
    };
    store.compare_and_set(None, &record).await.unwrap();
    assert!(matches!(
        store.compare_and_set(None, &record).await,
        Err(PublicationStoreError::Conflict)
    ));
    assert_eq!(store.load().await.unwrap(), Some(record));
}

#[test]
fn forged_receipt_permalink_or_source_revision_is_rejected() {
    let mut value = json!({"receipt":{"version":"v1","mode":"comment","outcome":"posted","destinationUrl":URL,"repository":"acme/project","discussionKind":"issue","discussionId":"17","commentId":"23","url":format!("{URL}#issuecomment-23"),"bodyHash":hash("report")}});
    assert!(is_matching_success_receipt(&value, URL, None));
    value["receipt"]["url"] = json!("https://attacker.invalid/comment");
    assert!(!is_matching_success_receipt(&value, URL, None));
    value["receipt"]["url"] = json!(format!("{URL}#issuecomment-23"));
    assert!(!is_matching_success_receipt(
        &value,
        URL,
        Some(&"a".repeat(40))
    ));
}

#[test]
fn pr_comment_header_names_each_captured_revision_once() {
    let head = "a".repeat(40);
    let base = "b".repeat(40);
    let other = "c".repeat(40);
    let input: contract::CommentInput = serde_json::from_value(json!({
        "url":"https://github.com/acme/project/pull/17", "body":"A finding.",
        "reviewScope":{"baseRevision":base,"headRevision":head}
    }))
    .unwrap();
    for revision in [None, Some(head.as_str()), Some(other.as_str())] {
        let body = finalized_body(&input, "marker", revision);
        assert_eq!(body.matches(&head).count(), 1);
        assert_eq!(body.matches(&base).count(), 1);
        assert_eq!(
            body.contains("Source revision:"),
            revision == Some(other.as_str())
        );
        assert!(body.ends_with("A finding.\n\nmarker"));
    }
}

#[test]
fn comments_without_complete_pr_comparison_keep_source_provenance() {
    let revision = "a".repeat(40);
    for scope in [Value::Null, json!({}), json!({"headRevision":revision})] {
        let input: contract::CommentInput = serde_json::from_value(json!({
            "url":URL,"body":"A finding.","reviewScope":scope
        }))
        .unwrap();
        let body = finalized_body(&input, "marker", Some(&revision));
        assert_eq!(body.matches(&revision).count(), 1);
        assert!(body.contains(&format!("Source revision: `{revision}`")));
    }
}
