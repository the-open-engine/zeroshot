use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

use openengine_cluster_protocol::{NodeName, WorkerRef};
use crate::native_v2_contract::{ExecutionId, ExecutionRef, NodeInstanceId};
use crate::native_v2_runner::{NodeResponseContract, NodeRole};
use crate::source_code_provider::SourceCommentPage;

use super::*;

const URL: &str = "https://github.com/acme/project/issues/17";

fn valid_issue_receipt() -> Value {
    json!({"receipt":{"version":"v1","mode":"comment","outcome":"posted","destinationUrl":URL,"repository":"acme/project","discussionKind":"issue","discussionId":"17","commentId":"23","url":format!("{URL}#issuecomment-23"),"bodyHash":hash("report")}})
}

#[test]
fn terminal_receipt_requires_the_exact_contract_and_discussion_identity() {
    let original = valid_issue_receipt();
    assert!(is_matching_success_receipt(&original, URL, None));
    for (field, value) in [
        ("version", json!("v2")),
        ("mode", json!("pull_request")),
        ("outcome", json!("unconfirmed")),
        ("bodyHash", json!("g".repeat(64))),
        (
            "destinationUrl",
            json!("https://github.com/acme/project/issues/18"),
        ),
        ("repository", json!("acme/other")),
        ("discussionKind", json!("pull_request")),
        ("discussionId", json!("18")),
        ("commentId", json!("0")),
        ("commentId", json!("not-numeric")),
        ("sourceRevision", json!("a".repeat(40))),
    ] {
        let mut changed = original.clone();
        changed["receipt"][field] = value;
        assert!(
            !is_matching_success_receipt(&changed, URL, None),
            "accepted changed {field}"
        );
    }
    assert!(!is_matching_success_receipt(&Value::Null, URL, None));
    assert!(!is_matching_success_receipt(
        &original,
        "https://other-forge.invalid/issues/17",
        None
    ));
    let mut unknown_field = original;
    unknown_field["receipt"]["unexpected"] = json!(true);
    assert!(!is_matching_success_receipt(&unknown_field, URL, None));
}

#[tokio::test]
async fn changed_native_response_contract_is_refused_before_authentication_or_intent() {
    let (_temp, forge, adapter) = fixture();
    for field in ["output", "signals", "diagnostic"] {
        let mut invocation = invocation(&adapter, json!({"url":URL,"body":"report"})).await;
        let NodeResponseContract::Verifier {
            output,
            signals,
            diagnostic,
        } = &mut invocation.response
        else {
            panic!("expected native verifier");
        };
        match field {
            "output" => *output = openengine_cluster_protocol::PayloadType::String,
            "signals" => signals.clear(),
            _ => *diagnostic = openengine_cluster_protocol::PayloadType::String,
        }
        assert_invalid_role_without_authority(&adapter, &forge, invocation).await;
    }
}

async fn assert_invalid_role_without_authority(
    adapter: &NativeV2CommentDeliveryAdapter,
    forge: &Forge,
    invocation: DriverInvocation,
) {
    let (_cancel, control, _durable) = crate::native_v2_runner::test_support::control();
    assert!(matches!(
        adapter.run(invocation, control).await,
        Err(NodeRunnerError::InvalidRole)
    ));
    assert_eq!(
        (
            forge.actors.load(Ordering::SeqCst),
            forge.creates.load(Ordering::SeqCst),
            adapter.store.load().await.unwrap()
        ),
        (0, 0, None)
    );
}

#[tokio::test]
async fn final_comment_size_includes_provenance_before_publication_authority_is_recorded() {
    let (_temp, forge, adapter) = fixture();
    let input = json!({"url":URL,"body":"x".repeat(MAX_BODY_BYTES)});
    let error = run(&adapter, input).await.unwrap_err();
    assert!(error.to_string().contains("final comment body exceeds"));
    assert_eq!(forge.creates.load(Ordering::SeqCst), 0);
    assert!(adapter.store.load().await.unwrap().is_none());
}

#[tokio::test]
async fn retained_ready_authority_cannot_change_destination_marker_actor_or_body() {
    for changed in ["url", "marker", "author", "body"] {
        let (_temp, forge, adapter) = fixture();
        let input = json!({"url":URL,"body":"report"});
        let invocation = invocation(&adapter, input.clone()).await;
        let mut retained = adapter
            .prepare_publication(
                &invocation,
                serde_json::from_value(input.clone()).unwrap(),
                "ACTOR_ID".into(),
            )
            .unwrap()
            .record;
        retained.status = PublicationStatus::Ready;
        match changed {
            "url" => retained.url = "https://github.com/acme/project/issues/18".into(),
            "marker" => {
                retained.marker = format!("<!-- zeroshot-comment:v1:{} -->", "c".repeat(64))
            }
            "author" => retained.author = "OTHER_ACTOR".into(),
            _ => retained.body_hash = "0".repeat(64),
        }
        adapter
            .store
            .compare_and_set(None, &retained)
            .await
            .unwrap();
        let error = run(&adapter, input).await.unwrap_err();
        assert!(error.to_string().contains(if changed == "body" {
            "comment_already_delivered"
        } else {
            "publication identity differs"
        }));
        assert_eq!(forge.creates.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.store.load().await.unwrap(), Some(retained));
    }
}

#[derive(Default)]
struct Forge {
    comments: Mutex<Vec<SourceComment>>,
    creates: AtomicUsize,
    actors: AtomicUsize,
    scans: AtomicUsize,
    lose_response: AtomicBool,
    omit_publication: AtomicBool,
    stale_scope: AtomicBool,
    actor_switch: AtomicBool,
    reject_posts: AtomicBool,
    reject_reads: AtomicBool,
    reject_actors: AtomicBool,
    reject_scope: AtomicBool,
    endless_pages: AtomicBool,
    unclean_post: AtomicBool,
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
        if self.reject_actors.swap(false, Ordering::SeqCst) {
            return Err(SourceDeliveryError::api(Some(401), "expired actor"));
        }
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
        self.scans.fetch_add(1, Ordering::SeqCst);
        if self.reject_reads.swap(false, Ordering::SeqCst) {
            return Err(SourceDeliveryError::api(Some(401), "expired"));
        }
        Ok(SourceCommentPage {
            comments: self.comments.lock().unwrap().clone(),
            has_more: self.endless_pages.load(Ordering::SeqCst),
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
        if self.unclean_post.load(Ordering::SeqCst) {
            return Err(unclean_command());
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
        if self.reject_scope.swap(false, Ordering::SeqCst) {
            return Err(SourceDeliveryError::api(Some(401), "expired scope"));
        }
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
    run_with_refreshed_input(adapter, json!({"url":URL,"body":"report"})).await
}

fn declared_environment(token: &str) -> (NodeRuntimeBinding, ResolvedEnvironment) {
    let binding: NodeRuntimeBinding = serde_json::from_value(
        json!({"kind":"comment_delivery","connections":{"github":["GH_TOKEN"]}}),
    )
    .unwrap();
    let environment = ResolvedEnvironment::exact(
        &binding,
        BTreeMap::from([(
            EnvironmentVariableName::new("GH_TOKEN").unwrap(),
            token.to_owned(),
        )]),
    )
    .unwrap();
    (binding, environment)
}

async fn run_with_refreshed_input(
    adapter: &NativeV2CommentDeliveryAdapter,
    input: Value,
) -> Result<WorkerOutcome, NodeRunnerError> {
    let mut invocation = invocation(adapter, input).await;
    let (binding, initial) = declared_environment("old-token");
    invocation.environment = crate::native_v2_runner::with_environment_refresh(
        initial,
        Arc::new(RotatedCredential(declared_environment("new-token").1)),
    );
    invocation.node.binding = binding;
    let (_cancel, control, _durable) = crate::native_v2_runner::test_support::control();
    adapter.run(invocation, control).await
}

#[tokio::test]
async fn renewed_post_authority_cannot_change_author_before_another_post() {
    let (_temp, forge, adapter) = fixture();
    forge.reject_posts.store(true, Ordering::SeqCst);
    forge.actor_switch.store(true, Ordering::SeqCst);
    let error = run_with_refresh(&adapter).await.unwrap_err();
    assert!(error.to_string().contains("changed the publication author"));
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
    assert!(matches!(
        publication_status(&adapter).await,
        PublicationStatus::Ready
    ));
}

#[tokio::test]
async fn expired_scan_credential_renews_once_and_retains_author_identity() {
    let (_temp, forge, adapter) = fixture();
    forge.reject_reads.store(true, Ordering::SeqCst);
    run_with_refresh(&adapter).await.unwrap();
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
    assert_eq!(forge.actors.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancellation_after_publication_retains_confirmation_for_successor() {
    let (_temp, forge, adapter) = fixture();
    let input = json!({"url":URL,"body":"report"});
    let invocation = invocation(&adapter, input.clone()).await;
    let (cancel, control, _durable) = crate::native_v2_runner::test_support::control();
    *forge.cancel_after_create.lock().unwrap() = Some(cancel);
    assert!(matches!(
        adapter.run(invocation, control).await,
        Err(NodeRunnerError::Cancelled)
    ));
    assert!(matches!(
        publication_status(&adapter).await,
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
    let path = private_root(&temp).join("publication.json");
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
        publication_status(&second).await,
        PublicationStatus::Confirmed { .. }
    ));
}

#[tokio::test]
async fn lost_post_response_reconciles_exact_author_marker_and_body() {
    let (_temp, forge, adapter) = fixture();
    forge.lose_response.store(true, Ordering::SeqCst);
    run(&adapter, json!({"url":URL,"body":"A report"}))
        .await
        .unwrap();
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
    assert!(matches!(
        publication_status(&adapter).await,
        PublicationStatus::Confirmed { .. }
    ));
}

#[tokio::test]
async fn uncertain_absent_post_is_never_blindly_repeated_after_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = private_root(&temp).join("publication.json");
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
    let (temp, forge, adapter) = fixture();
    let error = run(
        &adapter,
        json!({"url":"https://github.com/acme/project/issues/18","body":"redirect"}),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("changed after admission"));
    assert_eq!(forge.actors.load(Ordering::SeqCst), 0);
    assert!(!private_root(&temp).join("publication.json").exists());
}

#[tokio::test]
async fn older_checkpoint_body_cannot_replace_a_delivered_report() {
    let (_temp, forge, adapter) = fixture();
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
    let (_temp, forge, adapter) = published_fixture().await;
    let input = json!({"url":URL,"body":"report"});
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
    let (_temp, forge, mut adapter) = fixture();
    forge.stale_scope.store(true, Ordering::SeqCst);
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
    let store = LocalCommentPublicationStore::new(private_root(&temp).join("publication.json"));
    let record = publication_record();
    store.compare_and_set(None, &record).await.unwrap();
    assert!(matches!(
        store.compare_and_set(None, &record).await,
        Err(PublicationStoreError::Conflict)
    ));
    assert_eq!(store.load().await.unwrap(), Some(record));
}

#[test]
fn forged_receipt_permalink_or_source_revision_is_rejected() {
    let mut value = valid_issue_receipt();
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

#[test]
fn case_variant_input_accepts_canonical_comment_permalink_without_relaxing_identity() {
    let destination = "https://github.com/ACME/PROJECT/pull/17";
    let mut value = json!({"receipt":{"version":"v1","mode":"comment","outcome":"posted","destinationUrl":destination,"repository":"ACME/PROJECT","discussionKind":"pull_request","discussionId":"17","commentId":"23","url":"https://github.com/Acme/Project/pull/17#issuecomment-23","bodyHash":hash("report")}});
    assert!(is_matching_success_receipt(&value, destination, None));
    for url in [
        "https://github.com/Acme/Other/pull/17#issuecomment-23",
        "https://github.com/Acme/Project/pull/18#issuecomment-23",
        "https://github.com/Acme/Project/pull/17#issuecomment-24",
        "https://github.com/Acme/Project/PULL/17#issuecomment-23",
        "https://github.com/Acme/Project/pull/17#ISSUECOMMENT-23",
        "https://github.com/Acme/Project/pull/17?redirect=1#issuecomment-23",
        "https://github.com.evil.test/Acme/Project/pull/17#issuecomment-23",
    ] {
        value["receipt"]["url"] = json!(url);
        assert!(!is_matching_success_receipt(&value, destination, None));
    }
}

type PublicationFixture = (
    tempfile::TempDir,
    Arc<Forge>,
    NativeV2CommentDeliveryAdapter,
);

fn fixture() -> PublicationFixture {
    let temp = tempfile::tempdir().unwrap();
    let forge = Arc::new(Forge::default());
    let adapter = adapter(private_root(&temp).join("publication.json"), forge.clone());
    (temp, forge, adapter)
}

fn private_root(temp: &tempfile::TempDir) -> std::path::PathBuf {
    let root = temp.path().join("private");
    crate::execution::platform::private_directory(&root).unwrap();
    root
}

async fn published_fixture() -> PublicationFixture {
    let (temp, forge, adapter) = fixture();
    run(&adapter, json!({"url":URL,"body":"report"}))
        .await
        .unwrap();
    (temp, forge, adapter)
}

async fn publication_status(adapter: &NativeV2CommentDeliveryAdapter) -> PublicationStatus {
    adapter.store.load().await.unwrap().unwrap().status
}

fn assert_calls(forge: &Forge, creates: usize, actors: usize) {
    assert_eq!(forge.creates.load(Ordering::SeqCst), creates);
    assert_eq!(forge.actors.load(Ordering::SeqCst), actors);
}

#[derive(Default)]
struct InterruptedStore {
    record: Mutex<Option<PublicationRecord>>,
    lose_claim_ack: AtomicBool,
    reject_confirmation: AtomicBool,
}

#[async_trait]
impl CommentPublicationStore for InterruptedStore {
    async fn load(&self) -> Result<Option<PublicationRecord>, PublicationStoreError> {
        Ok(self.record.lock().unwrap().clone())
    }

    async fn compare_and_set(
        &self,
        expected: Option<&PublicationRecord>,
        next: &PublicationRecord,
    ) -> Result<(), PublicationStoreError> {
        let mut current = self.record.lock().unwrap();
        if current.as_ref() != expected {
            return Err(PublicationStoreError::Conflict);
        }
        if matches!(next.status, PublicationStatus::Confirmed { .. })
            && self.reject_confirmation.swap(false, Ordering::SeqCst)
        {
            return Err(PublicationStoreError::Unavailable(
                "confirmation failed".into(),
            ));
        }
        *current = Some(next.clone());
        if self.lose_claim_ack.swap(false, Ordering::SeqCst) {
            return Err(PublicationStoreError::Unavailable(
                "claim acknowledgement lost".into(),
            ));
        }
        Ok(())
    }
}

#[tokio::test]
async fn confirmed_nonpublication_allows_one_post_retry_with_renewed_authority() {
    let (_temp, forge, adapter) = fixture();
    forge.reject_posts.store(true, Ordering::SeqCst);
    run_with_refresh(&adapter).await.unwrap();
    assert_eq!(forge.creates.load(Ordering::SeqCst), 2);
    assert_eq!(forge.comments.lock().unwrap().len(), 1);
    assert!(matches!(
        publication_status(&adapter).await,
        PublicationStatus::Confirmed { .. }
    ));
}

#[tokio::test]
async fn confirmed_comment_read_renews_without_reposting() {
    let (_temp, forge, adapter) = published_fixture().await;
    forge.reject_reads.store(true, Ordering::SeqCst);
    run_with_refresh(&adapter).await.unwrap();
    assert_calls(&forge, 1, 3);
}

#[tokio::test]
async fn initial_actor_authentication_renews_before_publication_identity_is_captured() {
    let (_temp, forge, adapter) = fixture();
    forge.reject_actors.store(true, Ordering::SeqCst);
    run_with_refresh(&adapter).await.unwrap();
    assert_calls(&forge, 1, 2);
}

#[tokio::test]
async fn captured_scope_read_can_renew_only_for_the_same_actor() {
    let (_temp, forge, mut adapter) = fixture();
    Arc::make_mut(&mut adapter.config).expected_review_scope = Some(json!({}));
    forge.reject_scope.store(true, Ordering::SeqCst);
    run_with_refreshed_input(
        &adapter,
        json!({"url":URL,"body":"report","reviewScope":{}}),
    )
    .await
    .unwrap();
    assert_calls(&forge, 1, 2);
}

#[tokio::test]
async fn failed_confirmation_is_reconciled_by_a_successor_without_a_second_post() {
    let (_temp, forge, mut adapter) = fixture();
    let store = Arc::new(InterruptedStore::default());
    store.reject_confirmation.store(true, Ordering::SeqCst);
    adapter.store = store.clone();
    let input = json!({"url":URL,"body":"report"});
    assert!(
        run(&adapter, input.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("confirmation failed")
    );
    assert!(matches!(
        publication_status(&adapter).await,
        PublicationStatus::Attempted
    ));
    run(&adapter, input).await.unwrap();
    assert!(matches!(
        publication_status(&adapter).await,
        PublicationStatus::Confirmed { .. }
    ));
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn uncertain_claim_acknowledgement_never_grants_a_post() {
    let (_temp, forge, mut adapter) = fixture();
    let store = Arc::new(InterruptedStore::default());
    store.lose_claim_ack.store(true, Ordering::SeqCst);
    adapter.store = store;
    let input = json!({"url":URL,"body":"report"});
    assert!(
        run(&adapter, input.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("acknowledgement lost")
    );
    assert!(
        run(&adapter, input)
            .await
            .unwrap_err()
            .to_string()
            .contains("prior create")
    );
    assert_eq!(forge.creates.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn bounded_scan_refuses_before_a_create_when_comments_never_end() {
    let (_temp, forge, adapter) = fixture();
    forge.endless_pages.store(true, Ordering::SeqCst);
    let error = run(&adapter, json!({"url":URL,"body":"report"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("page bound"));
    assert_eq!(
        forge.scans.load(Ordering::SeqCst),
        MAX_COMMENT_PAGES as usize
    );
    assert_eq!(forge.creates.load(Ordering::SeqCst), 0);
    assert!(adapter.store.load().await.unwrap().is_none());
}

#[tokio::test]
async fn duplicate_remote_publication_identity_cannot_choose_an_arbitrary_receipt() {
    let (_temp, forge, adapter) = published_fixture().await;
    let input = json!({"url":URL,"body":"report"});
    let mut duplicate = forge.comments.lock().unwrap()[0].clone();
    duplicate.id = "24".into();
    duplicate.url = format!("{URL}#issuecomment-24");
    forge.comments.lock().unwrap().push(duplicate);
    let mut successor = adapter.clone();
    successor.store = Arc::new(InterruptedStore::default());
    let error = run(&successor, input).await.unwrap_err();
    assert!(error.to_string().contains("multiple comments"));
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
    assert!(successor.store.load().await.unwrap().is_none());
}

fn unclean_command() -> SourceDeliveryError {
    crate::source_code_provider::GitCommandFailure {
        command: "gh api".into(),
        working_directory: ".".into(),
        exit_status: None,
        stdout: String::new(),
        stderr: "process cleanup was not confirmed".into(),
        stdout_truncated: false,
        stderr_truncated: false,
        context: "comment transport".into(),
        timed_out: false,
        cleanup_unconfirmed: true,
        classification: None,
    }
    .into()
}

#[tokio::test]
async fn unconfirmed_transport_cleanup_retains_intent_and_never_scans_after_create() {
    let (_temp, forge, adapter) = fixture();
    forge.unclean_post.store(true, Ordering::SeqCst);
    assert!(matches!(
        run(&adapter, json!({"url":URL,"body":"report"})).await,
        Err(NodeRunnerError::CleanupUnconfirmed)
    ));
    assert_eq!(forge.scans.load(Ordering::SeqCst), 1);
    assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
    assert!(matches!(
        publication_status(&adapter).await,
        PublicationStatus::Attempted
    ));
}

#[tokio::test]
async fn invalid_or_oversized_comment_content_never_reaches_a_post() {
    for input in [
        Value::Null,
        json!({"url":URL}),
        json!({"url":URL,"body":"  "}),
        json!({"url":URL,"body":"report <!-- zeroshot-comment:v1:forged -->"}),
        json!({"url":URL,"body":"x".repeat(MAX_BODY_BYTES + 1)}),
        json!({"url":URL,"body":"x".repeat(MAX_BODY_BYTES)}),
    ] {
        let (_temp, forge, adapter) = fixture();
        assert!(run(&adapter, input).await.is_err());
        assert_eq!(forge.creates.load(Ordering::SeqCst), 0);
        assert!(adapter.store.load().await.unwrap().is_none());
    }
}

#[tokio::test]
async fn invalid_declared_credential_cannot_fall_back_to_trusted_authority() {
    for token in [
        String::new(),
        "two words".into(),
        "é".into(),
        "x".repeat(4097),
    ] {
        let (_temp, forge, adapter) = fixture();
        let adapter = adapter.with_trusted_github_token(Some(Arc::from("fallback-token")));
        let mut invocation = invocation(&adapter, json!({"url":URL,"body":"report"})).await;
        let (binding, environment) = declared_environment(&token);
        invocation.node.binding = binding;
        invocation.environment = environment;
        let (_cancel, control, _durable) = crate::native_v2_runner::test_support::control();
        let error = adapter.run(invocation, control).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("declared comment credential is invalid")
        );
        assert_eq!(forge.actors.load(Ordering::SeqCst), 0);
        assert_eq!(forge.creates.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn closed_or_wrong_role_sessions_cannot_authorize_publication() {
    for closed in [false, true] {
        let (_temp, forge, adapter) = fixture();
        let mut invocation = invocation(&adapter, json!({"url":URL,"body":"report"})).await;
        if closed {
            invocation.session.close().await;
            assert!(!invocation.session.is_live().await);
        } else {
            invocation.role = NodeRole::Worker;
        }
        assert_invalid_role_without_authority(&adapter, &forge, invocation).await;
    }
}

#[tokio::test]
async fn pr_receipt_retains_exact_source_revision_and_comparison_identity() {
    let (_temp, forge, mut adapter) = fixture();
    let url = "https://github.com/acme/project/pull/17";
    let revision = "a".repeat(40);
    let scope = json!({"baseRepository":"acme/project","baseBranch":"main","baseRevision":"b".repeat(40),
        "headRepository":"acme/project","headBranch":"fix","headRevision":revision});
    let config = Arc::make_mut(&mut adapter.config);
    config.expected_url = url.into();
    config.expected_review_scope = Some(scope.clone());
    config.source_revision = Some(revision.clone());
    let result = run(
        &adapter,
        json!({"url":url,"body":"report","reviewScope":scope}),
    )
    .await
    .unwrap();
    let WorkerOutcome::Verifier { output, .. } = result else {
        panic!("expected receipt")
    };
    assert!(is_matching_success_receipt(&output, url, Some(&revision)));
    assert_eq!(output["receipt"]["discussionKind"], "pull_request");
    assert_eq!(forge.comments.lock().unwrap().len(), 1);
}

fn publication_record() -> PublicationRecord {
    PublicationRecord {
        version: 1,
        url: URL.into(),
        marker: marker(&RunId::new("root"), "deliver"),
        body_hash: hash("report"),
        author: "ACTOR_ID".into(),
        status: PublicationStatus::Attempted,
    }
}

fn write_private(path: &std::path::Path, bytes: &[u8]) {
    use std::io::Write as _;
    let mut file = crate::execution::platform::private_file(
        path,
        crate::execution::platform::FileAccess::CreateNew,
    )
    .unwrap();
    file.write_all(bytes).unwrap();
}

fn token_file(temp: &tempfile::TempDir) -> (std::path::PathBuf, TrustedGitHubTokenFile) {
    let root = private_root(temp);
    let workspace = root.join("checkout");
    crate::execution::platform::private_directory(&workspace).unwrap();
    let path = root.join("token");
    let file = TrustedGitHubTokenFile::new(path.clone(), &workspace).unwrap();
    (path, file)
}

#[tokio::test]
async fn trusted_file_rotation_is_bounded_and_declared_authority_wins() {
    let (temp, _forge, adapter) = fixture();
    let (path, file) = token_file(&temp);
    write_private(&path, b"old-token\n");
    let adapter = adapter
        .with_trusted_github_token(Some(Arc::from("fallback-token")))
        .with_trusted_github_token_file(Some(file));
    let invocation = invocation(&adapter, json!({"url":URL,"body":"report"})).await;
    let mut credentials = Credentials::new(&adapter, &invocation.environment).unwrap();
    assert_eq!(credentials.current().expose(), "old-token");
    std::fs::write(&path, b"new-token\n").unwrap();
    assert!(credentials.refresh().await.unwrap());
    assert_eq!(credentials.current().expose(), "new-token");
    assert!(!credentials.refresh().await.unwrap());
    let (_, declared) = declared_environment("declared-token");
    let mut credentials = Credentials::new(&adapter, &declared).unwrap();
    assert_eq!(credentials.current().expose(), "declared-token");
    assert!(!credentials.refresh().await.unwrap());
}

#[tokio::test]
async fn missing_or_unrotated_trusted_file_refuses_authority() {
    let (temp, forge, adapter) = fixture();
    let (path, file) = token_file(&temp);
    let adapter = adapter.with_trusted_github_token_file(Some(file));
    let input = json!({"url":URL,"body":"report"});
    let error = run(&adapter, input.clone()).await.unwrap_err();
    assert!(error.to_string().contains("credential file is unavailable"));
    assert_eq!(forge.actors.load(Ordering::SeqCst), 0);
    write_private(&path, b"unchanged-token");
    let invocation = invocation(&adapter, input).await;
    let mut credentials = Credentials::new(&adapter, &invocation.environment).unwrap();
    assert!(
        credentials
            .refresh()
            .await
            .unwrap_err()
            .to_string()
            .contains("refresh unavailable")
    );
    assert!(!credentials.refresh().await.unwrap());
}

struct UnavailableRefresh;

#[async_trait]
impl crate::native_v2_runner::RuntimeEnvironmentRefresh for UnavailableRefresh {
    async fn refresh(
        &self,
    ) -> Result<ResolvedEnvironment, crate::native_v2_runner::EnvironmentRefreshError> {
        Err(crate::native_v2_runner::EnvironmentRefreshError::Unavailable)
    }
}

#[tokio::test]
async fn failed_or_invalid_environment_refresh_cannot_reuse_rejected_authority() {
    let (_temp, _forge, adapter) = fixture();
    let empty = invocation(&adapter, json!({"url":URL,"body":"report"}))
        .await
        .environment;
    let refreshers: Vec<Arc<dyn crate::native_v2_runner::RuntimeEnvironmentRefresh>> = vec![
        Arc::new(UnavailableRefresh),
        Arc::new(RotatedCredential(empty)),
        Arc::new(RotatedCredential(declared_environment("invalid token").1)),
    ];
    for refresh in refreshers {
        let initial = crate::native_v2_runner::with_environment_refresh(
            declared_environment("old-token").1,
            refresh,
        );
        let mut credentials = Credentials::new(&adapter, &initial).unwrap();
        assert!(credentials.refresh().await.is_err());
        assert!(!credentials.refresh().await.unwrap());
    }
}

#[test]
fn credential_errors_redact_authority_and_preserve_cleanup_failure() {
    let (_temp, _forge, adapter) = fixture();
    let (_, environment) = declared_environment("private-test-token");
    let credentials = Credentials::new(&adapter, &environment).unwrap();
    let error = credentials.failure(SourceDeliveryError::api(
        None,
        "rejected private-test-token",
    ));
    assert!(!error.to_string().contains("private-test-token"));
    assert!(error.to_string().contains("[REDACTED]"));
    assert!(matches!(
        credentials.failure(unclean_command()),
        NodeRunnerError::CleanupUnconfirmed
    ));
}

#[test]
fn retained_publication_cannot_adopt_another_destination_marker_author_or_body() {
    let intended = publication_record();
    for previous in [
        PublicationRecord {
            url: "https://github.com/acme/project/issues/18".into(),
            ..intended.clone()
        },
        PublicationRecord {
            marker: marker(&RunId::new("other"), "deliver"),
            ..intended.clone()
        },
        PublicationRecord {
            author: "OTHER_ACTOR".into(),
            ..intended.clone()
        },
        PublicationRecord {
            body_hash: hash("different report"),
            ..intended.clone()
        },
        PublicationRecord {
            version: 2,
            ..intended.clone()
        },
    ] {
        assert!(validate_previous(&previous, &intended).is_err());
    }
}

#[tokio::test]
async fn corrupt_oversized_or_invalid_publication_storage_fails_closed() {
    let mut invalid_record = publication_record();
    invalid_record.version = 2;
    for bytes in [
        b"not-json".to_vec(),
        vec![b'x'; 16 * 1024 + 1],
        serde_json::to_vec(&invalid_record).unwrap(),
    ] {
        let (temp, forge, adapter) = fixture();
        write_private(&private_root(&temp).join("publication.json"), &bytes);
        let error = run(&adapter, json!({"url":URL,"body":"report"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("publication record is invalid"));
        assert_eq!(forge.creates.load(Ordering::SeqCst), 0);
    }
    let temp = tempfile::tempdir().unwrap();
    let path = private_root(&temp).join("parent");
    std::fs::write(&path, "not a directory").unwrap();
    let store = LocalCommentPublicationStore::new(path.join("publication.json"));
    assert!(matches!(
        store.load().await,
        Err(PublicationStoreError::Unavailable(_))
    ));
    assert!(matches!(
        store.compare_and_set(None, &publication_record()).await,
        Err(PublicationStoreError::Unavailable(_))
    ));
    let store = LocalCommentPublicationStore::new(std::path::PathBuf::new());
    assert!(matches!(
        store.load().await,
        Err(PublicationStoreError::InvalidRecord)
    ));
}

#[tokio::test]
async fn mismatched_response_contract_or_worker_refuses_before_authority() {
    let (_temp, forge, adapter) = fixture();
    let mut turn = invocation(&adapter, json!({"url":URL,"body":"report"})).await;
    assert!(turn.session.as_any().is::<CommentSession>());
    turn.response = NodeResponseContract::Worker {
        output: openengine_cluster_protocol::PayloadType::Null,
    };
    let (_cancel, control, _durable) = crate::native_v2_runner::test_support::control();
    assert!(matches!(
        adapter.run(turn, control).await,
        Err(NodeRunnerError::InvalidRole)
    ));
    let mut node = invocation(&adapter, json!({"url":URL,"body":"report"}))
        .await
        .node;
    let (_, environment) = declared_environment("token");
    node.worker = WorkerRef::new("builtin.other@1").unwrap();
    assert!(matches!(
        adapter.open(&node, &environment).await,
        Err(NodeRunnerError::InvalidRole)
    ));
    assert_eq!(forge.actors.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unavailable_renewal_never_authorizes_an_extra_comment_post() {
    for operation in ["actor", "scan", "create", "receipt"] {
        let (_temp, forge, adapter) = fixture();
        let input = json!({"url":URL,"body":"report"});
        match operation {
            "actor" => forge.reject_actors.store(true, Ordering::SeqCst),
            "scan" => forge.reject_reads.store(true, Ordering::SeqCst),
            "create" => forge.reject_posts.store(true, Ordering::SeqCst),
            _ => {
                run(&adapter, input.clone()).await.unwrap();
                forge.reject_reads.store(true, Ordering::SeqCst);
            }
        }
        let error = run(&adapter, input).await.unwrap_err();
        assert!(
            error.to_string().contains("expired"),
            "{operation}: {error}"
        );
        let expected_posts = usize::from(matches!(operation, "create" | "receipt"));
        assert_eq!(forge.creates.load(Ordering::SeqCst), expected_posts);
        let record = adapter.store.load().await.unwrap();
        match operation {
            "create" => assert_eq!(record.unwrap().status, PublicationStatus::Ready),
            "receipt" => assert!(matches!(
                record.unwrap().status,
                PublicationStatus::Confirmed { .. }
            )),
            _ => assert!(record.is_none()),
        }
    }
}

#[tokio::test]
async fn untrusted_forge_actor_and_confirmed_comment_identity_cannot_grant_a_receipt() {
    let (_temp, forge, adapter) = fixture();
    let input = json!({"url":URL,"body":"report"});
    let turn = invocation(&adapter, input.clone()).await;
    for author in [
        String::new(),
        "two actors".into(),
        "é".into(),
        "x".repeat(257),
    ] {
        let error = adapter
            .prepare_publication(
                &turn,
                serde_json::from_value(input.clone()).unwrap(),
                author,
            )
            .err()
            .expect("invalid actor must refuse publication");
        assert!(
            error
                .to_string()
                .contains("invalid comment author identity")
        );
    }
    let publication = adapter
        .prepare_publication(
            &turn,
            serde_json::from_value(input).unwrap(),
            "ACTOR_ID".into(),
        )
        .unwrap();
    let mut credentials = Credentials::new(&adapter, &turn.environment).unwrap();
    let (_cancel, control, _durable) = crate::native_v2_runner::test_support::control();
    assert!(
        !adapter
            .retry_authentication(
                &publication,
                &SourceDeliveryError::api(Some(403), "permission denied"),
                (&mut credentials, &control),
            )
            .await
            .unwrap()
    );
    assert_eq!(forge.creates.load(Ordering::SeqCst), 0);
    assert!(adapter.store.load().await.unwrap().is_none());

    let corruptions: [fn(&mut SourceComment); 4] = [
        |comment| comment.author = "OTHER_ACTOR".into(),
        |comment| comment.discussion_url = "https://github.com/acme/project/issues/18".into(),
        |comment| comment.url = "https://attacker.invalid/comment".into(),
        |comment| comment.id = "deleted-comment".into(),
    ];
    for corrupt in corruptions {
        let (_temp, forge, adapter) = published_fixture().await;
        corrupt(&mut forge.comments.lock().unwrap()[0]);
        assert!(
            run(&adapter, json!({"url":URL,"body":"report"}))
                .await
                .is_err()
        );
        assert_eq!(forge.creates.load(Ordering::SeqCst), 1);
        assert!(matches!(
            publication_status(&adapter).await,
            PublicationStatus::Confirmed { .. }
        ));
    }
}
