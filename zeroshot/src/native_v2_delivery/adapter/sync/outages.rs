use super::*;
use crate::native_v2_runner::{NativeNodeRunner, NodeRunner, RuntimeEnvironmentRefresh};
use crate::native_v2_runner::test_support;
use openengine_cluster_protocol::{DeclaredConnections, DeclaredEnvironment};
use openengine_cluster_testkit::assertions::AssertValue;
use std::sync::atomic::AtomicUsize;
use tokio::time::Instant;

#[derive(Clone, Copy)]
enum Outage {
    Api,
    Refresh,
    Visibility,
    BranchVisibility(usize),
}

struct OutageAuthority {
    outage: Outage,
    available_at: Instant,
    calls: AtomicUsize,
}

#[async_trait]
impl SourceDeliveryAuthority for OutageAuthority {
    async fn open_or_update_review(
        &self,
        _request: &SourceReviewRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewReceipt, SourceDeliveryError> {
        let attempt = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Outage::BranchVisibility(failures) = self.outage
            && attempt <= failures
        {
            return Err(SourceDeliveryError::api(
                Some(400),
                format!(
                    "GitLab merge request creation for branch zeroshot/outage: source_branch does not exist; echoed credential {}",
                    credential.expose(),
                ),
            ).temporary_review_sync());
        }
        let failure = match self.outage {
            Outage::Refresh if credential.expose() != "refreshed-token" => Some(401),
            Outage::Api if Instant::now() < self.available_at => Some(503),
            Outage::Visibility if Instant::now() < self.available_at => Some(422),
            _ => None,
        };
        if let Some(status) = failure {
            return Err(SourceDeliveryError::api(
                Some(status),
                format!("HTTP {status} scripted outage"),
            ));
        }
        Ok(SourceReviewReceipt {
            review_id: "17".to_owned(),
            repository: "acme/project".to_owned(),
            target_branch: "main".to_owned(),
            head_branch: "zeroshot/outage".to_owned(),
            head_revision: "b".repeat(40),
        })
    }

    async fn observe_delivery(
        &self,
        _: SourceDeliveryRead<'_>,
        _: SourceDeliveryCredential<'_>,
    ) -> Result<SourceDeliverySnapshot, SourceDeliveryError> {
        Err(SourceDeliveryError::Rejected)
    }

    async fn reconcile_delivery_target(
        &self,
        _: SourceTargetReconciliation<'_>,
        _: SourceDeliveryCredential<'_>,
    ) -> Result<SourceTargetIntegration, SourceDeliveryError> {
        Err(SourceDeliveryError::Rejected)
    }

    async fn reconcile_delivery_head(
        &self,
        _: SourceHeadReconciliation<'_>,
        _: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
        Err(SourceDeliveryError::Rejected)
    }

    async fn push_branch(
        &self,
        _: &SourcePushRequest,
        _: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        Err(SourceDeliveryError::Rejected)
    }

    async fn inspect_review(
        &self,
        _: &SourceReviewReceipt,
        _: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewObservation, SourceDeliveryError> {
        Err(SourceDeliveryError::Rejected)
    }

    async fn request_merge(
        &self,
        _: &SourceReviewReceipt,
        _: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
        Err(SourceDeliveryError::Rejected)
    }
}

struct TimedRefresh {
    available_at: Instant,
    environment: ResolvedEnvironment,
    calls: AtomicUsize,
}

#[async_trait]
impl RuntimeEnvironmentRefresh for TimedRefresh {
    async fn refresh(&self) -> Result<ResolvedEnvironment, EnvironmentRefreshError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if Instant::now() < self.available_at {
            return Err(EnvironmentRefreshError::Unavailable);
        }
        Ok(self.environment.clone())
    }
}

struct SyncProbe {
    adapter: NativeV2DeliveryAdapter,
    request: SourceReviewRequest,
    environment: ResolvedEnvironment,
    starts: AtomicUsize,
}

#[async_trait]
impl SessionFactory for SyncProbe {
    async fn open(
        &self,
        _: &NodeInvocation,
        _: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        Ok(Arc::new(DeliverySession {
            workspace: std::env::temp_dir(),
            live: AtomicBool::new(true),
        }))
    }
}

#[async_trait]
impl NodeDriver for SyncProbe {
    async fn run(
        &self,
        _: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let mut credentials = DeliveryCredentials {
            environment: Some(&self.environment),
            token_file: None,
            token: "test-token".to_owned(),
            git: None,
            forge_field: Some(GITHUB_TOKEN_ENV),
            github_git_token: "test-token".to_owned(),
            token_from_environment: true,
        };
        match self
            .adapter
            .synchronize_review(&self.request, &mut credentials, &control)
            .await
        {
            Ok(review) => {
                assert_eq!(review.head_revision, self.request.head_revision);
                Ok(WorkerOutcome::Verified {
                    output: Value::Null,
                    artifacts: Vec::new(),
                })
            }
            Err(stop) => stop.result(),
        }
    }
}

fn token_environment(token: &str) -> ResolvedEnvironment {
    let name = EnvironmentVariableName::new(GITHUB_TOKEN_ENV).assert_value();
    let binding = NodeRuntimeBinding::GitDelivery {
        connections: DeclaredConnections::single(
            "github",
            DeclaredEnvironment::new([name.clone()]).assert_value(),
        )
        .assert_value(),
        pull_request_feedback: Default::default(),
    };
    ResolvedEnvironment::exact(&binding, BTreeMap::from([(name, token.to_owned())])).assert_value()
}

fn probe(outage: Outage) -> (Arc<SyncProbe>, Arc<OutageAuthority>, Arc<TimedRefresh>) {
    let available_at = Instant::now() + Duration::from_secs(90);
    let authority = Arc::new(OutageAuthority {
        outage,
        available_at,
        calls: AtomicUsize::new(0),
    });
    let refresh = Arc::new(TimedRefresh {
        available_at,
        environment: token_environment("refreshed-token"),
        calls: AtomicUsize::new(0),
    });
    let request = SourceReviewRequest {
        include_terminal_reviews: true,
        known_review: None,
        target: DeliveryTarget::new("acme/project", "main", "a".repeat(40)).assert_value(),
        head_branch: "zeroshot/outage".to_owned(),
        head_revision: "b".repeat(40),
        title: "delivery".to_owned(),
        description: "test".to_owned(),
        source_issue: None,
    };
    let adapter = NativeV2DeliveryAdapter::new(
        NativeV2DeliveryConfig {
            local_branch: None,
            delivery_run_id: openengine_cluster_protocol::RunId::new("outage-test"),
            adopt_existing_delivery: false,
            git_identity: None,
            commit_identity: None,
            commit_co_author: None,
            workspace: std::env::temp_dir(),
            git_program: PathBuf::from("unused-git"),
            target: request.target.clone(),
            poll: DeliveryPollPolicy::until_cancelled(Duration::from_secs(1)),
        },
        authority.clone(),
    );
    let mut environment = token_environment("test-token");
    if matches!(outage, Outage::Refresh) {
        environment =
            crate::native_v2_runner::with_environment_refresh(environment, refresh.clone());
    }
    (
        Arc::new(SyncProbe {
            adapter,
            request,
            environment,
            starts: AtomicUsize::new(0),
        }),
        authority,
        refresh,
    )
}

async fn run_probe(probe: Arc<SyncProbe>) -> WorkerOutcome {
    run_probe_with_output(probe).await.0
}

async fn run_probe_with_output(probe: Arc<SyncProbe>) -> (WorkerOutcome, String) {
    let runner = NativeNodeRunner::new(&test_support::admitted(), probe.clone(), probe.clone())
        .assert_value();
    let mut handle = runner
        .start(test_support::request("sync-outage", "worker", (1, 1)))
        .await
        .assert_value();
    let mut output = handle.attach();
    let completion = handle.completion().await.assert_value();
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
    let mut text = String::new();
    while let Ok(output) = output.recv().await {
        for secret in ["test-token", "refreshed-token"] {
            assert!(!output.text.contains(secret), "{}", output.text);
        }
        text.push_str(&output.text);
    }
    (completion.outcome, text)
}

#[tokio::test(start_paused = true)]
async fn http_503_outage_longer_than_a_sync_batch_recovers_in_one_execution() {
    let (probe, authority, _) = probe(Outage::Api);
    assert_recovers_after_the_outage(probe).await;
    assert!(authority.calls.load(Ordering::SeqCst) > REVIEW_SYNC_ATTEMPTS);
}

#[tokio::test(start_paused = true)]
async fn refresh_outage_past_sync_deadline_does_not_become_authentication_refusal() {
    let (probe, authority, refresh) = probe(Outage::Refresh);
    assert_recovers_after_the_outage(probe).await;
    assert!(authority.calls.load(Ordering::SeqCst) >= 3);
    assert!(refresh.calls.load(Ordering::SeqCst) > 1);
}

#[tokio::test(start_paused = true)]
async fn visibility_failures_keep_the_existing_bounded_batch() {
    let (probe, authority, _) = probe(Outage::Visibility);
    let started = Instant::now();
    assert_eq!(
        run_probe(probe).await,
        WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
    );
    assert!(started.elapsed() < Duration::from_secs(90));
    assert_eq!(authority.calls.load(Ordering::SeqCst), REVIEW_SYNC_ATTEMPTS);
}

#[tokio::test(start_paused = true)]
async fn missing_source_branch_keeps_the_existing_attempt_count_and_interval() {
    let (probe, authority, _) = probe(Outage::BranchVisibility(usize::MAX));
    let started = Instant::now();
    let (outcome, output) = run_probe_with_output(probe).await;
    assert_eq!(
        outcome,
        WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
    );
    assert_eq!(authority.calls.load(Ordering::SeqCst), REVIEW_SYNC_ATTEMPTS);
    assert_eq!(started.elapsed(), Duration::from_secs(55));
    assert!(output.contains("failed after 10 attempt(s)"), "{output}");
    assert!(output.contains("source_branch does not exist"), "{output}");
    assert!(output.contains("zeroshot/outage"), "{output}");
}

#[tokio::test(start_paused = true)]
async fn missing_source_branch_recovers_without_a_worker_or_reviewer_round() {
    let (probe, authority, _) = probe(Outage::BranchVisibility(1));
    let started = Instant::now();
    assert!(matches!(
        run_probe(probe).await,
        WorkerOutcome::Verified { .. }
    ));
    assert_eq!(authority.calls.load(Ordering::SeqCst), 2);
    assert_eq!(started.elapsed(), REVIEW_SYNC_INTERVAL);
}

async fn assert_recovers_after_the_outage(probe: Arc<SyncProbe>) {
    let started = Instant::now();
    assert!(matches!(
        run_probe(probe).await,
        WorkerOutcome::Verified { .. }
    ));
    assert!(started.elapsed() >= Duration::from_secs(90));
}
