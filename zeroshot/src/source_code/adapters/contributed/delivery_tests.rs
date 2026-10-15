//! A contributed forge exercises the existing PR and merge nodes with opaque identities.
use super::*;
use crate::source_code_provider::adapters::core::{
    SourceDeliveryFactoryRequest, delivery_authority_with_registry,
    validate_delivery_source_with_registry,
};
use crate::source_code_provider::provider::*;
use crate::native_v2_delivery::transport::{GitTransport, GitTransportConfig};
use openengine_cluster_testkit::assertions::AssertValue;
use std::sync::Mutex;
use std::path::Path;
use std::sync::atomic::AtomicBool;

const REPOSITORY: &str = "tenant:17/projects/team/nested/repo";
const REVIEW: &str = "reviews:opaque/@482-f";
const TOKEN_FIELD: &str = "FORGE_API_TOKEN";

struct ContributedForge {
    git: crate::source_code_provider::adapters::core::GitDeliveryAuthority,
    remote: PathBuf,
    review: Mutex<Option<SourceReviewReceipt>>,
    merged: AtomicBool,
}

#[async_trait]
impl SourceDeliveryAuthority for ContributedForge {
    fn forge_credential_field(&self) -> Option<&'static str> {
        Some(TOKEN_FIELD)
    }
    async fn observe_delivery(
        &self,
        request: SourceDeliveryRead<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceDeliverySnapshot, SourceDeliveryError> {
        if credential.expose() != "contributed-token-v2" {
            return Err(SourceDeliveryError::api(
                Some(401),
                "forge rejected the expired fixture token",
            ));
        }
        let mut snapshot = self
            .git
            .observe_delivery(
                SourceDeliveryRead {
                    include_review: false,
                    ..request
                },
                credential,
            )
            .await?;
        snapshot.review = self.review.lock().unwrap().as_ref().map(|r| {
            r.observation(SourceReviewState::Open {
                checks: SourceChecks::Passed,
            })
        });
        Ok(snapshot)
    }
    async fn reconcile_delivery_target(
        &self,
        request: SourceTargetReconciliation<'_>,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceTargetIntegration, SourceDeliveryError> {
        Ok(SourceTargetIntegration {
            target_revision: request.target.base_revision.clone(),
            outcome: SourceReconciliationOutcome::Unchanged,
        })
    }
    async fn reconcile_delivery_head(
        &self,
        request: SourceHeadReconciliation<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReconciliationOutcome, SourceDeliveryError> {
        self.git.reconcile_delivery_head(request, credential).await
    }
    async fn push_branch(
        &self,
        request: &SourcePushRequest,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        self.git.push_branch(request, credential).await
    }
    async fn open_or_update_review(
        &self,
        request: &SourceReviewRequest,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewReceipt, SourceDeliveryError> {
        let allocated = SourceReviewReceipt {
            repository: REPOSITORY.to_owned(),
            review_id: REVIEW.to_owned(),
            head_revision: request.head_revision.clone(),
            head_branch: request.head_branch.clone(),
            target_branch: request.target.target_branch.clone(),
        };
        self.review.lock().unwrap().replace(allocated.clone());
        Ok(allocated)
    }
    async fn inspect_review(
        &self,
        review: &SourceReviewReceipt,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceReviewObservation, SourceDeliveryError> {
        let mut observation = review.observation_with_readiness(
            SourceReviewState::Open {
                checks: SourceChecks::Passed,
            },
            false,
            false,
        );
        if self.merged.load(Ordering::SeqCst) {
            observation.state = SourceReviewState::Merged {
                merge_revision: git_output(&self.remote, &["rev-parse", "refs/heads/main"]),
            };
        }
        Ok(observation)
    }
    async fn request_merge(
        &self,
        review: &SourceReviewReceipt,
        _credential: SourceDeliveryCredential<'_>,
    ) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
        git(
            &self.remote,
            &["update-ref", "refs/heads/main", &review.head_revision],
        );
        self.merged.store(true, Ordering::SeqCst);
        Ok(SourceMergeRequestOutcome::Accepted)
    }
}

struct Contribution {
    descriptor: SourceProviderDescriptor,
    authority: Arc<dyn SourceDeliveryAuthority>,
}
fn unsupported() -> SourceProviderFailure {
    SourceProviderFailure::new(
        SourceProviderFailureCode::InvalidRequest,
        SourceFailureMessage::new("fixture exposes trusted-delivery capabilities").assert_value(),
    )
    .assert_value()
}
#[async_trait]
impl SourceCodeProvider for Contribution {
    fn descriptor(&self) -> &SourceProviderDescriptor {
        &self.descriptor
    }
    fn delivery_authority(&self) -> Option<Arc<dyn SourceDeliveryAuthority>> {
        Some(self.authority.clone())
    }
    async fn identify_repository(
        &self,
        _request: &SourceIdentifyRepositoryRequest,
    ) -> Result<CanonicalRepository, SourceProviderFailure> {
        Err(unsupported())
    }
    async fn inspect_repository(
        &self,
        _request: &SourceInspectRepositoryRequest,
    ) -> Result<SourceRepositoryInspection, SourceProviderFailure> {
        Err(unsupported())
    }
    async fn materialize(
        &self,
        _request: &SourceMaterializeRequest,
        _destination: SourceMaterializationDestination<'_>,
    ) -> Result<SourceMaterializationReceipt, SourceProviderFailure> {
        Err(unsupported())
    }
    async fn inspect_operation(
        &self,
        _request: &SourceOperationRequest,
    ) -> Result<SourceOperationInspection, SourceProviderFailure> {
        Err(unsupported())
    }
    async fn operate(
        &self,
        _request: &SourceOperationRequest,
        _workspace: SourceWorkspaceCapability<'_>,
    ) -> Result<SourceOperationReceipt, SourceProviderFailure> {
        Err(unsupported())
    }
}

fn registry(repo: &TempRepo) -> (SourceCodeProviderRegistry, Arc<ContributedForge>) {
    let transport = GitTransport::new(
        GitTransportConfig {
            git_program: "/usr/bin/git".into(),
            workspace: repo.workspace.clone(),
            home_directory: repo.root.path().to_owned(),
            private_directory: repo.root.path().to_owned(),
            identity: None,
            local_environment: None,
            deadline: Duration::from_secs(10),
        },
        &repo.remote,
    );
    let authority = Arc::new(ContributedForge {
        git: crate::source_code_provider::adapters::core::GitDeliveryAuthority::new(transport),
        remote: repo.remote.clone(),
        review: Mutex::new(None),
        merged: AtomicBool::new(false),
    });
    let descriptor = SourceProviderDescriptor::new(
        SourceProviderRef::new(
            SourceProviderId::new("source.contributed-fixture").assert_value(),
            1,
        )
        .assert_value(),
        BTreeMap::from([(
            SourceProfileId::new("default").assert_value(),
            SourceProfileDescriptor::new(
                [
                    SourceCapability::Read,
                    SourceCapability::Push,
                    SourceCapability::PullRequest,
                    SourceCapability::Checks,
                    SourceCapability::Merge,
                ]
                .into_iter()
                .collect(),
                Default::default(),
            )
            .assert_value(),
        )]),
    )
    .assert_value();
    let mut registry = SourceCodeProviderRegistry::new();
    registry
        .register(Arc::new(Contribution {
            descriptor,
            authority: authority.clone(),
        }))
        .assert_value();
    (registry, authority)
}

#[tokio::test]
async fn registered_contribution_executes_existing_pr_and_merge_nodes_with_opaque_ids() {
    for mode in [DeliveryMode::PullRequestV2, DeliveryMode::Merge] {
        let repo = TempRepo::delivery();
        let change = format!("candidate implementation for {}\n", mode.label());
        fs::write(repo.workspace.join("contributed-change.txt"), &change).assert_value();
        let source: ResolvedSource = serde_json::from_value(json!({
            "repository": "git-source:opaque", "branch": "main", "revision": repo.base,
            "remote": {"url":"https://forge.invalid/nested/team/repo.git", "forge":{
                "provider":"source.contributed-fixture", "repository":REPOSITORY}}
        }))
        .assert_value();
        let (registry, forge) = registry(&repo);
        validate_delivery_source_with_registry(&source, mode.required_capabilities(), &registry)
            .assert_value();
        let authority = delivery_authority_with_registry(
            SourceDeliveryFactoryRequest {
                source: &source,
                workspace: &repo.workspace,
                git_program: Path::new("/usr/bin/git"),
                gh_program: Path::new("/usr/bin/false"),
                home_directory: repo.root.path(),
                private_directory: repo.root.path(),
                git_identity: None,
                local_environment: None,
                required_capabilities: mode.required_capabilities(),
                operator_diagnostics: None,
            },
            &registry,
        )
        .assert_value();
        let adapter = NativeV2DeliveryAdapter::new(
            NativeV2DeliveryConfig {
                delivery_run_id: RunId::new("contributed-forge"),
                adopt_existing_delivery: false,
                workspace: repo.workspace.clone(),
                git_program: "/usr/bin/git".into(),
                git_identity: None,
                target: source_target(&source, mode).assert_value(),
                poll: DeliveryPollPolicy::new(3, Duration::ZERO).assert_value(),
            },
            authority,
        );
        let (stored, resolutions) =
            run_contributed(&source, mode, Arc::new(adapter), &registry).await;
        assert_eq!(stored.admitted.source, Some(source.clone()));
        assert!(matches!(
            stored.snapshot.terminal,
            Some(TerminalResult::Succeeded { .. })
        ));
        assert!(
            resolutions >= 2,
            "forge authentication must refresh its own declared field"
        );
        let review = forge.review.lock().unwrap().clone().assert_value();
        assert_eq!(review.repository, REPOSITORY);
        assert_eq!(review.review_id, REVIEW);
        assert_ne!(
            review.head_revision, repo.base,
            "receipt must certify new candidate work"
        );
        assert_eq!(
            git_output(
                &repo.remote,
                &[
                    "show",
                    &format!("{}:contributed-change.txt", review.head_revision)
                ]
            ),
            change.trim()
        );
        if mode.is_merge() {
            assert!(forge.merged.load(Ordering::SeqCst));
            assert_eq!(
                git_output(&repo.remote, &["rev-parse", "refs/heads/main"]),
                review.head_revision
            );
        }
    }
}

struct ForgeResolver {
    resolutions: AtomicUsize,
}
#[async_trait]
impl crate::native_v2_supervisor::RunConnectionResolver for ForgeResolver {
    async fn resolve(
        &self,
        requirements: openengine_cluster_protocol::RunConnectionRequirements,
    ) -> Result<
        openengine_cluster_protocol::RunConnectionValues,
        crate::native_v2_supervisor::ConnectionResolutionError,
    > {
        let attempt = self.resolutions.fetch_add(1, Ordering::SeqCst);
        let token = if attempt == 0 {
            "contributed-token-v1"
        } else {
            "contributed-token-v2"
        };
        Ok(requirements
            .into_iter()
            .map(|(key, fields)| {
                assert_eq!(
                    fields,
                    [EnvironmentVariableName::new(TOKEN_FIELD).assert_value()]
                        .into_iter()
                        .collect::<Vec<_>>()
                );
                (
                    key,
                    StaticConnectionValues::new(BTreeMap::from([(
                        EnvironmentVariableName::new(TOKEN_FIELD).assert_value(),
                        token.to_owned(),
                    )]))
                    .assert_value(),
                )
            })
            .collect())
    }
}

async fn run_contributed(
    source: &ResolvedSource,
    mode: DeliveryMode,
    adapter: Arc<NativeV2DeliveryAdapter>,
    registry: &SourceCodeProviderRegistry,
) -> (crate::v2_run_ledger::StoredRun, usize) {
    let binding = NodeRuntimeBinding::GitDelivery {
        connections: DeclaredConnections::single(
            "forge",
            DeclaredEnvironment::new([EnvironmentVariableName::new(TOKEN_FIELD).assert_value()])
                .assert_value(),
        )
        .assert_value(),
        pull_request_feedback: PullRequestFeedback::Consider,
    };
    let (graph, initial_input) = contribution_graph(mode);
    let admitted = NativeV2Admission
        .admit_with_source_registry(
            RunSubmission {
                environment: None,
                title: RunTitle::new("Contributed forge").assert_value(),
                graph,
                initial_input,
                runtime: delivery_runtime(binding),
                source: Some(source.clone()),
                submission_key: IdempotencyKey::new(format!("contributed-{}", mode.label()))
                    .assert_value(),
            },
            registry,
        )
        .await
        .assert_value();
    let resolver = Arc::new(ForgeResolver {
        resolutions: AtomicUsize::new(0),
    });
    let environment = Arc::new(
        RunEnvironment::with_resolver_for_admitted(
            &admitted,
            BTreeMap::new(),
            crate::native_v2_supervisor::DynamicConnectionPlan {
                resolver: resolver.clone(),
                keys: [ConnectionKey::new("forge").assert_value()]
                    .into_iter()
                    .collect(),
                source_connection: None,
            },
        )
        .assert_value(),
    );
    let runner =
        Arc::new(NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value());
    let ledger = Arc::new(FakeRunLedger::new());
    let run_id = RunId::new("contributed-forge");
    ledger
        .create_or_get(CreateRun {
            run_id: run_id.clone(),
            submission_key: IdempotencyKey::new("contributed-forge").assert_value(),
            submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
            admitted,
        })
        .await
        .assert_value();
    let terminal = NativeV2Supervisor::new(run_id.clone(), ledger.clone(), runner, environment)
        .drive()
        .await
        .assert_value();
    let stored = ledger.get(&run_id).await.assert_value().assert_value();
    assert!(
        matches!(terminal, TerminalResult::Succeeded { .. }),
        "actual contributed source must pass supervisor delivery accounting: {terminal:?}; snapshot: {:?}",
        stored.snapshot,
    );
    (stored, resolver.resolutions.load(Ordering::SeqCst))
}

fn contribution_graph(mode: DeliveryMode) -> (openengine_cluster_protocol::GraphSpec, Value) {
    let schema = serde_json::to_value(delivery_result_schema(mode).assert_value()).assert_value();
    let fields = schema["fields"].as_object().assert_value();
    let mut delivery = delivery_node(mode);
    delivery["writeBindings"] = Value::Array(
        fields
            .keys()
            .map(|field| {
                json!({
                    "target":[field], "value":{"node":"deliver","channel":"out","path":[field]}
                })
            })
            .collect(),
    );
    let terminal = json!({
        "kind":"succeed", "name":"done", "output":schema,
        "bindings":fields.keys().map(|field| json!({
            "target":[field], "value":{"source":"state","path":[field]}
        })).collect::<Vec<_>>()
    });
    let initial = Value::Object(
        fields
            .iter()
            .map(|(name, field)| {
                let value = match field["type"]["kind"].as_str() {
                    Some("enum") => field["type"]["values"][0].clone(),
                    Some("string") => Value::String(String::new()),
                    other => panic!("unexpected contribution receipt field: {other:?}"),
                };
                (name.clone(), value)
            })
            .collect(),
    );
    let mut graph = serde_json::to_value(full_graph(vec![delivery, terminal])).assert_value();
    graph["initialInput"] = schema.clone();
    graph["root"]["state"] = schema;
    (serde_json::from_value(graph).assert_value(), initial)
}
