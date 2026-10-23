use super::*;
use zeroshot_engine::native_v2_delivery::{
    SourceDeliveryRead, SourceDeliverySnapshot, SourceHeadReconciliation,
    SourceReconciliationOutcome, SourceTargetIntegration, SourceTargetReconciliation,
};

#[derive(Clone, Copy)]
pub(crate) enum DeliveryScenario {
    NoCi,
    CiFailureThenMerge,
}

pub(crate) struct DeliveryFixture {
    pub(crate) remote: PathBuf,
    pub(crate) workspace: PathBuf,
    pub(crate) base_revision: String,
}

impl DeliveryFixture {
    pub(crate) fn new(root: &TempRoot, name: &str) -> Self {
        let remote = root.path(&format!("{name}-remote.git"));
        let seed = root.path(&format!("{name}-seed"));
        let workspace = root.path(&format!("{name}-workspace"));
        git(
            root.as_path(),
            &[
                "init",
                "--initial-branch=main",
                seed.to_str().assert_value(),
            ],
        );
        std::fs::write(seed.join("README.md"), "base\n").assert_value();
        git(&seed, &["add", "README.md"]);
        git(&seed, &["config", "user.name", "Test"]);
        git(&seed, &["config", "user.email", "test@example.invalid"]);
        git(&seed, &["commit", "--message=base"]);
        git(
            root.as_path(),
            &[
                "clone",
                "--bare",
                seed.to_str().assert_value(),
                remote.to_str().assert_value(),
            ],
        );
        git(
            root.as_path(),
            &[
                "clone",
                remote.to_str().assert_value(),
                workspace.to_str().assert_value(),
            ],
        );
        let base_revision = git_output(&workspace, &["rev-parse", "HEAD"]);
        std::fs::write(workspace.join("result.txt"), "ready for delivery\n").assert_value();
        Self {
            remote,
            workspace,
            base_revision,
        }
    }
}

fn git(directory: &Path, arguments: &[&str]) {
    assert!(
        git_command(directory)
            .args(arguments)
            .status()
            .assert_value()
            .success()
    );
}

fn git_output(directory: &Path, arguments: &[&str]) -> String {
    let output = git_command(directory)
        .args(arguments)
        .output()
        .assert_value();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn git_command(directory: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("/usr/bin/git");
    command.arg("-C").arg(directory);
    command
}

pub(crate) struct DeliveryAuthority {
    pub(crate) remote: PathBuf,
    scenario: DeliveryScenario,
    pub(crate) reviews: AtomicUsize,
    pub(crate) inspections: AtomicUsize,
    pub(crate) merge_requests: AtomicUsize,
    merged_reviews: Mutex<BTreeSet<String>>,
}

impl DeliveryAuthority {
    pub(crate) fn new(remote: PathBuf, scenario: DeliveryScenario) -> Self {
        Self {
            remote,
            scenario,
            reviews: AtomicUsize::new(0),
            inspections: AtomicUsize::new(0),
            merge_requests: AtomicUsize::new(0),
            merged_reviews: Mutex::new(BTreeSet::new()),
        }
    }
}

fn require_test_credential(
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    (credential.expose() == "test-token")
        .then_some(())
        .ok_or(SourceDeliveryError::Rejected)
}

fn review_receipt(request: &SourceReviewRequest, review_id: String) -> SourceReviewReceipt {
    SourceReviewReceipt {
        review_id,
        repository: request.target.repository.clone(),
        target_branch: request.target.target_branch.clone(),
        head_branch: request.head_branch.clone(),
        head_revision: request.head_revision.clone(),
    }
}

fn review_observation(
    review: &SourceReviewReceipt,
    state: SourceReviewState,
) -> SourceReviewObservation {
    SourceReviewObservation {
        state,
        pull_request_ready: false,
        head_update_required: false,
        head_revision: review.head_revision.clone(),
        head_branch: review.head_branch.clone(),
        target_branch: review.target_branch.clone(),
        repository: review.repository.clone(),
        review_id: review.review_id.clone(),
    }
}

#[async_trait]
impl SourceDeliveryAuthority for DeliveryAuthority {
    fn forge_credential_field(&self) -> Option<&'static str> {
        Some(GITHUB_TOKEN_ENV)
    }

    async fn observe_delivery(
        &self,
        request: SourceDeliveryRead<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceDeliverySnapshot, SourceDeliveryError> {
        require_test_credential(credential)?;
        let output = git_command(&self.remote)
            .args(["rev-parse", "--verify", "--quiet"])
            .arg(format!("refs/heads/{}", request.head_branch))
            .output()
            .map_err(|_| SourceDeliveryError::Unavailable)?;
        let head_revision = match output.status.code() {
            Some(0) => Some(String::from_utf8_lossy(&output.stdout).trim().to_owned()),
            Some(1) => None,
            _ => return Err(SourceDeliveryError::Rejected),
        };
        let review = match request.known_review {
            Some(known) => Some(self.inspect_review(known, credential).await?),
            None => None,
        };
        Ok(SourceDeliverySnapshot {
            review,
            head_revision,
        })
    }

    async fn reconcile_delivery_target(
        &self,
        request: SourceTargetReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceTargetIntegration, SourceDeliveryError> {
        require_test_credential(credential)?;
        let target_revision = git_output(
            &self.remote,
            &[
                "rev-parse",
                &format!("refs/heads/{}", request.target.target_branch),
            ],
        );
        // This fixture advances neither the target branch nor its admitted source.
        assert_eq!(target_revision, request.target.base_revision);
        git(
            request.workspace,
            &["merge-base", "--is-ancestor", &target_revision, "HEAD"],
        );
        Ok(SourceTargetIntegration {
            target_revision,
            outcome: SourceReconciliationOutcome::Unchanged,
        })
    }

    async fn reconcile_delivery_head(
        &self,
        request: SourceHeadReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
        require_test_credential(credential)?;
        // This fixture models CI repair without remote branch edits.
        assert_eq!(request.published, request.observed);
        assert!(
            git_command(request.workspace)
                .args([
                    "merge-base",
                    "--is-ancestor",
                    &request.observed.head_revision,
                    "HEAD"
                ])
                .status()
                .assert_value()
                .success()
        );
        Ok(SourceReconciliationOutcome::Unchanged)
    }

    async fn push_branch(
        &self,
        request: &SourcePushRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        require_test_credential(credential)?;
        let mut push = tokio::process::Command::new("/usr/bin/git");
        push.arg("-C")
            .arg(&request.workspace)
            .args(["push", self.remote.to_str().assert_value()])
            .arg(format!("HEAD:refs/heads/{}", request.head_branch));
        match push.status().await {
            Ok(status) if status.success() => Ok(()),
            Ok(_) => Err(SourceDeliveryError::Rejected),
            Err(_) => Err(SourceDeliveryError::Unavailable),
        }
    }

    async fn open_or_update_review(
        &self,
        request: &SourceReviewRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewReceipt, SourceDeliveryError> {
        require_test_credential(credential)?;
        let review_id = (self.reviews.fetch_add(1, Ordering::SeqCst) + 1).to_string();
        Ok(review_receipt(request, review_id))
    }

    async fn inspect_review(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewObservation, SourceDeliveryError> {
        require_test_credential(credential)?;
        self.inspections.fetch_add(1, Ordering::SeqCst);
        let merged = self
            .merged_reviews
            .lock()
            .assert_value()
            .contains(&review.review_id);
        let state = if merged {
            SourceReviewState::Merged {
                merge_revision: review.head_revision.clone(),
            }
        } else if matches!(self.scenario, DeliveryScenario::CiFailureThenMerge)
            && review.review_id == "1"
        {
            SourceReviewState::Open {
                checks: SourceChecks::Failed {
                    diagnostic: "Required CI checks failed:\n- realistic CI fixture failed"
                        .to_owned(),
                },
            }
        } else {
            SourceReviewState::Open {
                checks: SourceChecks::NotRequired,
            }
        };
        Ok(review_observation(review, state))
    }

    async fn request_merge(
        &self,
        review: &SourceReviewReceipt,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
        require_test_credential(credential)?;
        self.merge_requests.fetch_add(1, Ordering::SeqCst);
        let mut merged = self.merged_reviews.lock().assert_value();
        merged.insert(review.review_id.clone());
        Ok(SourceMergeRequestOutcome::Accepted)
    }
}

struct DeliveryLane {
    delivery: NativeV2DeliveryAdapter,
    workspace: PathBuf,
    repairs: Arc<AtomicUsize>,
}

#[async_trait]
impl SessionFactory for DeliveryLane {
    async fn open(
        &self,
        invocation: &NodeInvocation,
        environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        match invocation.binding {
            NodeRuntimeBinding::CommentDelivery { .. } => Err(NodeRunnerError::InvalidRole),
            NodeRuntimeBinding::GitDelivery { .. } => {
                SessionFactory::open(&self.delivery, invocation, environment).await
            }
            NodeRuntimeBinding::Agent { .. } => Ok(Arc::new(ImmediateSession {
                live: std::sync::atomic::AtomicBool::new(true),
            })),
        }
    }
}

#[async_trait]
impl NodeDriver for DeliveryLane {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        if matches!(
            invocation.node.binding,
            NodeRuntimeBinding::GitDelivery { .. }
        ) {
            return NodeDriver::run(&self.delivery, invocation, control).await;
        }
        let repair = self.repairs.fetch_add(1, Ordering::SeqCst) + 1;
        std::fs::write(
            self.workspace.join(format!("repair-{repair}.txt")),
            format!("repair {repair}\n"),
        )
        .map_err(|_| NodeRunnerError::Driver)?;
        Ok(WorkerOutcome::Verified {
            output: serde_json::Value::Null,
            artifacts: Vec::new(),
        })
    }
}

pub(crate) struct DeliveryAllocator {
    pub(crate) fixture: DeliveryFixture,
    pub(crate) authority: Arc<DeliveryAuthority>,
    pub(crate) repairs: Arc<AtomicUsize>,
    pub(crate) lifecycle: ImmediateAllocator,
}

#[async_trait]
impl CapsuleAllocator for DeliveryAllocator {
    async fn claim_controller(
        &self,
        run_id: &RunId,
    ) -> Result<Arc<dyn ExclusiveControllerClaim>, ControllerClaimUnavailable> {
        self.lifecycle.claim_controller(run_id).await
    }

    async fn allocate(
        &self,
        request: zeroshot_engine::native_v2_platform::CapsuleAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        let run_id = request.run_id;
        let admitted = request.admitted;
        let source = admitted
            .source
            .as_ref()
            .ok_or(CapsuleAllocationUnavailable::Runtime)?;
        let delivery = NativeV2DeliveryAdapter::new(
            NativeV2DeliveryConfig {
                delivery_run_id: run_id.clone(),
                adopt_existing_delivery: false,
                git_identity: None,
                workspace: self.fixture.workspace.clone(),
                git_program: PathBuf::from("/usr/bin/git"),
                target: DeliveryTarget::new(
                    source.repository.as_str(),
                    source.branch.as_str(),
                    source.revision.as_str(),
                )
                .map_err(|_| CapsuleAllocationUnavailable::Runtime)?,
                poll: DeliveryPollPolicy::new(3, Duration::ZERO)
                    .map_err(|_| CapsuleAllocationUnavailable::Runtime)?,
            },
            self.authority.clone(),
        );
        let lane = Arc::new(DeliveryLane {
            delivery,
            workspace: self.fixture.workspace.clone(),
            repairs: self.repairs.clone(),
        });
        let runner = NativeNodeRunner::new(admitted, lane.clone(), lane)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
        let (sender, loss) = watch::channel(false);
        self.lifecycle.losses.lock().assert_value().push(sender);
        Ok(AllocatedCapsule {
            checkpoints: None,
            execution_seed: Vec::new(),
            runner: Arc::new(runner),
            loss,
            cleanup: Arc::new(ImmediateCleanup),
        })
    }

    async fn destroy_or_confirm_absent(
        &self,
        run_id: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        self.lifecycle.destroy_or_confirm_absent(run_id, exit).await
    }
}

use openengine_cluster_testkit::assertions::{AssertValue};
