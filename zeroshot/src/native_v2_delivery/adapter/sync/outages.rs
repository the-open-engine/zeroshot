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
}

struct OutageAuthority {
    outage: Outage,
    available_at: Instant,
    calls: AtomicUsize,
}

#[async_trait]
impl GitHubDeliveryAuthority for OutageAuthority {
    async fn open_or_update_review(
        &self,
        _request: &GitHubReviewRequest,
        credential: GitHubCredential<'_>,
    ) -> Result<GitHubReviewReceipt, GitHubAuthorityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let failure = match self.outage {
            Outage::Refresh if credential.expose() != "refreshed-token" => Some(401),
            Outage::Api if Instant::now() < self.available_at => Some(503),
            Outage::Visibility if Instant::now() < self.available_at => Some(422),
            _ => None,
        };
        if let Some(status) = failure {
            return Err(GitHubAuthorityError::api(
                Some(status),
                format!("HTTP {status} scripted outage"),
            ));
        }
        Ok(GitHubReviewReceipt {
            review_id: "17".to_owned(),
            repository: "acme/project".to_owned(),
            target_branch: "main".to_owned(),
            head_branch: "zeroshot/outage".to_owned(),
            head_revision: "b".repeat(40),
        })
    }

    async fn observe_delivery(
        &self,
        _: GitHubDeliveryRead<'_>,
        _: GitHubCredential<'_>,
    ) -> Result<GitHubDeliverySnapshot, GitHubAuthorityError> {
        Err(GitHubAuthorityError::Rejected)
    }

    async fn reconcile_delivery_head(
        &self,
        _: GitHubHeadReconciliation<'_>,
        _: GitHubCredential<'_>,
    ) -> Result<GitHubReconciliationOutcome, GitHubAuthorityError> {
        Err(GitHubAuthorityError::Rejected)
    }

    async fn push_branch(
        &self,
        _: &GitHubPushRequest,
        _: GitHubCredential<'_>,
    ) -> Result<(), GitHubAuthorityError> {
        Err(GitHubAuthorityError::Rejected)
    }

    async fn inspect_review(
        &self,
        _: &GitHubReviewReceipt,
        _: GitHubCredential<'_>,
    ) -> Result<GitHubReviewObservation, GitHubAuthorityError> {
        Err(GitHubAuthorityError::Rejected)
    }

    async fn request_merge(
        &self,
        _: &GitHubReviewReceipt,
        _: GitHubCredential<'_>,
    ) -> Result<GitHubMergeRequestOutcome, GitHubAuthorityError> {
        Err(GitHubAuthorityError::Rejected)
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
    request: GitHubReviewRequest,
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
            token: "test-token".to_owned(),
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
    let request = GitHubReviewRequest {
        target: DeliveryTarget::new("acme/project", "main", "a".repeat(40)).assert_value(),
        head_branch: "zeroshot/outage".to_owned(),
        head_revision: "b".repeat(40),
        title: "delivery".to_owned(),
        description: "test".to_owned(),
        source_issue: None,
    };
    let adapter = NativeV2DeliveryAdapter::new(
        NativeV2DeliveryConfig {
            delivery_run_id: openengine_cluster_protocol::RunId::new("outage-test"),
            adopt_existing_delivery: false,
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
    let runner = NativeNodeRunner::new(&test_support::admitted(), probe.clone(), probe.clone())
        .assert_value();
    let mut handle = runner
        .start(test_support::request("sync-outage", "worker", (1, 1)))
        .await
        .assert_value();
    let completion = handle.completion().await.assert_value();
    assert_eq!(probe.starts.load(Ordering::SeqCst), 1);
    completion.outcome
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

async fn assert_recovers_after_the_outage(probe: Arc<SyncProbe>) {
    let started = Instant::now();
    assert!(matches!(
        run_probe(probe).await,
        WorkerOutcome::Verified { .. }
    ));
    assert!(started.elapsed() >= Duration::from_secs(90));
}
