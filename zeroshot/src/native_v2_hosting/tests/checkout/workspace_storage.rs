use super::*;
use crate::native_v2_candidate::test_support::NodeRequestFixture;
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
use crate::native_v2_comment_delivery::{
    COMMENT_DELIVERY_WORKER_REF, CommentPublicationStore, LocalCommentPublicationStore,
    PublicationStatus,
};
use crate::native_v2_supervisor::checkpoints::{
    CheckpointError, ResticCheckpointStore, ResticCheckpointStoreTestConfig,
};

struct ExternalStorage {
    reject: bool,
    comment_publications: Option<Arc<dyn CommentPublicationStore>>,
}

#[async_trait::async_trait]
impl HostedWorkspaceStorage for ExternalStorage {
    async fn prepare(
        &self,
        request: HostedWorkspaceRequest<'_>,
    ) -> Result<HostedWorkspace, CheckpointError> {
        assert!(
            request.workspace.join(".git/HEAD").is_file(),
            "source must already be installed"
        );
        if self.reject {
            return Err(std::io::Error::other("restore failed").into());
        }
        fs::write(
            request.workspace.join("restored.txt"),
            b"durable platform bytes",
        )?;
        Ok(HostedWorkspace {
            comment_publications: self.comment_publications.clone(),
            checkpoints: Arc::new(ResticCheckpointStore::with_program(
                ResticCheckpointStoreTestConfig {
                    directory: request.checkpoint_directory.to_owned(),
                    repository: request.checkpoint_directory.join("repository"),
                    workspace: request.workspace.to_owned(),
                    writers: request.writers,
                    program: crate::native_v2_supervisor::checkpoints::restic::ResticProgram::fake(
                        request
                            .workspace
                            .parent()
                            .ok_or_else(|| std::io::Error::other("workspace has no parent"))?,
                    )?,
                },
            )),
            execution_seed: Vec::new(),
            // Recovery keeps the historical identity even after the product rename.
            delivery_run_id: Some(RunId::new("original-cloud-delivery")),
        })
    }
}

#[tokio::test]
async fn external_workspace_restore_precedes_dispatch_and_preserves_delivery_lineage() {
    let fixture = CheckoutFixture::with_workspace_storage(
        "",
        Some(Arc::new(ExternalStorage {
            reject: false,
            comment_publications: None,
        })),
    )
    .await;
    let capsule = fixture.allocate().await.assert_value();
    let workspace = fixture
        .allocator
        .run_path(&fixture.run_id)
        .join("workspace");
    assert_eq!(
        fs::read(workspace.join("restored.txt")).assert_value(),
        b"durable platform bytes"
    );
    assert!(capsule.checkpoints.is_some());
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    assert_eq!(
        fixture.allocator.recovery_delivery_run_id(&fixture.run_id),
        Some(RunId::new("original-cloud-delivery"))
    );
}

#[tokio::test]
async fn external_restore_failure_never_releases_an_allocated_capsule() {
    let fixture = CheckoutFixture::with_workspace_storage(
        "",
        Some(Arc::new(ExternalStorage {
            reject: true,
            comment_publications: None,
        })),
    )
    .await;
    assert!(matches!(
        fixture.allocate().await,
        Err(CapsuleAllocationUnavailable::Runtime)
    ));
    assert!(!fixture.allocator.run_path(&fixture.run_id).exists());
}

const COMMENT_URL: &str = "https://github.com/acme/project/pull/17";
const COMMENT_GH: &str = r#"#!/usr/bin/env python3
import json, os, pathlib, sys
assert os.environ['GH_TOKEN'] == 'checkout-secret'
assert sys.argv[1] == 'api'
fixture = pathlib.Path(sys.argv[0])
endpoint = sys.argv[2]
posted = fixture.with_suffix('.posted')
if endpoint == 'graphql':
    result = {'data': {'viewer': {'id': 'HOST_ACTOR'}}}
elif endpoint == 'repos/acme/project/pulls/17':
    result = json.loads(fixture.with_suffix('.scope').read_text())
elif endpoint == 'repos/acme/project/issues/17/comments?per_page=100&page=1':
    result = [json.loads(posted.read_text())] if posted.exists() else []
elif endpoint == 'repos/acme/project/issues/17/comments':
    assert sys.argv[3:] == ['--method', 'POST', '--input', '-']
    result = {'id': 23, 'html_url': 'https://github.com/acme/project/pull/17#issuecomment-23',
              'issue_url': 'https://api.github.com/repos/acme/project/issues/17',
              'body': json.load(sys.stdin)['body'], 'user': {'node_id': 'HOST_ACTOR'}}
    posted.write_text(json.dumps(result))
    with fixture.with_suffix('.posts').open('a') as count: count.write('x')
elif endpoint == 'repos/acme/project/issues/comments/23':
    result = json.loads(posted.read_text())
else:
    raise AssertionError(endpoint)
print(json.dumps(result))
"#;

async fn comment_fixture(
    host_supplied_store: bool,
) -> (CheckoutFixture, LocalCommentPublicationStore) {
    let mut fixture = CheckoutFixture::new("").await;
    let scope = json!({
        "baseRepository":"acme/project", "baseBranch":"main",
        "baseRevision":fixture._repository.main_revision,
        "headRepository":"acme/project", "headBranch":"feature",
        "headRevision":fixture._repository.feature_revision,
    });
    let runtime = serde_json::from_value(json!({
        "harness":"codex","provider":"openai","size":"small",
        "activeMemory":{"repository":false,"graph":false},
        "globalAgents":{"postmortem":{"enabled":false}},
        "nodes":{
            "classifier":{"kind":"agent","model":"fixture-model"},
            "reproducer":{"kind":"agent","model":"fixture-model"},
            "code_checker":{"kind":"agent","model":"fixture-model"},
            "deliver":{"kind":"comment_delivery","connections":{"github":["GH_TOKEN"]}}
        }
    }))
    .assert_value();
    let mut request = submission(
        runtime,
        &fixture._repository.feature_revision,
        "hosted-comment",
    );
    crate::native_v2_candidate::test_support::git(
        &fixture._repository.remote,
        &[
            "update-ref",
            "refs/pull/17/head",
            &fixture._repository.feature_revision,
        ],
    );
    request.graph = BuiltinGraphTemplate::PrReview
        .materialize(TemplateDelivery::Comment)
        .assert_value();
    request.initial_input = json!({
        "url":COMMENT_URL,
        "context":{"title":"Scoped PR", "body":"Review this", "reviewScope":scope,
                   "linkedIssues":[]}
    });
    fixture.admitted = admit(request).await;
    let program = fixture.root.write_executable(
        "gh-fixture",
        &openengine_cluster_testkit::fixture::python3_script(COMMENT_GH),
    );
    fs::write(
        program.with_extension("scope"),
        serde_json::to_vec(&json!({
            "base":{"repo":{"full_name":"acme/project"},"ref":"main",
                    "sha":fixture._repository.main_revision},
            "head":{"repo":{"full_name":"acme/project"},"ref":"feature",
                    "sha":fixture._repository.feature_revision}
        }))
        .assert_value(),
    )
    .assert_value();
    let fallback = fixture
        .root
        .child("comment-publications/original-cloud-delivery.json");
    let store = LocalCommentPublicationStore::new(if host_supplied_store {
        fixture
            .root
            .child("cloud-publications/authorized-slot.json")
    } else {
        fallback
    });
    let mut config = capsule_config(fixture.root.path().to_owned());
    config.gh_program = program;
    config.workspace_storage = Some(Arc::new(ExternalStorage {
        reject: false,
        comment_publications: host_supplied_store
            .then(|| Arc::new(store.clone()) as Arc<dyn CommentPublicationStore>),
    }));
    fixture.allocator =
        ProductionCapsuleAllocator::new(Arc::new(config), fixture.diagnostics.clone())
            .assert_value()
            .with_test_filesystem_and_source(
                fixture._repository.remote.clone(),
                portable_filesystem,
            );
    (fixture, store)
}

async fn dispatch_comment(
    fixture: &CheckoutFixture,
    capsule: &AllocatedCapsule,
    input: Value,
    execution: u64,
) -> Result<Value, crate::native_v2_runner::NodeRunnerError> {
    let node = NodeName::new("deliver").assert_value();
    let mut request = NodeRequestFixture {
        run_id: fixture.run_id.as_str(),
        node: "deliver",
        node_instance: 1,
        execution,
        worker: COMMENT_DELIVERY_WORKER_REF,
        instructions: "Unused native-node instructions",
        input,
        binding: fixture.admitted.runtime.nodes()[&node].clone(),
        environment: BTreeMap::from([(environment_name("GH_TOKEN"), CHECKOUT_TOKEN.to_owned())]),
    }
    .into_request();
    request.invocation.instructions = None;
    let mut handle = capsule.runner.start(request).await?;
    let outcome = handle.completion().await?.outcome;
    let openengine_cluster_protocol::WorkerOutcome::Verifier { output, .. } = outcome else {
        panic!("native publisher must return a verifier receipt");
    };
    Ok(output)
}

struct QuietCommentPreparation;

#[async_trait::async_trait]
impl crate::native_v2_platform::PreparationProgress for QuietCommentPreparation {
    async fn log(&self, _line: &str) -> Result<(), CapsuleAllocationUnavailable> {
        Ok(())
    }
}

async fn allocate_comment(fixture: &CheckoutFixture) -> AllocatedCapsule {
    let environment = RunEnvironment::from_available(
        &fixture.admitted.runtime,
        None,
        &BTreeMap::from([(environment_name("GH_TOKEN"), CHECKOUT_TOKEN.to_owned())]),
    )
    .assert_value();
    let result = fixture
        .allocator
        .allocate(crate::native_v2_platform::CapsuleAllocationRequest {
            run_id: &fixture.run_id,
            admitted: &fixture.admitted,
            github_token: Some(CHECKOUT_TOKEN),
            git_credentials: None,
            preparation: crate::native_v2_platform::CapsulePreparation {
                environment,
                progress: Arc::new(QuietCommentPreparation),
            },
        })
        .await;
    if let Err(error) = &result {
        panic!(
            "comment allocation failed: {error:?}; {:?}",
            fixture.diagnostics.snapshot(&fixture.run_id)
        );
    }
    result.assert_value()
}

#[tokio::test]
async fn hosted_comment_candidate_uses_authorized_store_scope_and_restored_delivery_lineage() {
    for host_supplied_store in [false, true] {
        let (mut fixture, store) = comment_fixture(host_supplied_store).await;
        let scope = fixture.admitted.initial_input["context"]["reviewScope"].clone();
        let input = json!({"url":COMMENT_URL,"body":"No actionable findings.","reviewScope":scope});
        let capsule = allocate_comment(&fixture).await;
        let mut changed = input.clone();
        changed["reviewScope"]["headRevision"] = json!("f".repeat(40));
        assert!(
            dispatch_comment(&fixture, &capsule, changed, 1)
                .await
                .is_err()
        );
        assert!(!fixture.root.child("gh-fixture.posted").exists());
        let first = dispatch_comment(&fixture, &capsule, input.clone(), 2)
            .await
            .assert_value();
        assert_eq!(
            first["receipt"]["sourceRevision"],
            fixture._repository.feature_revision
        );
        assert_eq!(first["receipt"]["destinationUrl"], COMMENT_URL);
        let publication = store.load().await.assert_value().assert_value();
        assert!(matches!(
            publication.status,
            PublicationStatus::Confirmed { .. }
        ));
        assert_eq!(
            fixture
                .root
                .child("comment-publications/original-cloud-delivery.json")
                .exists(),
            !host_supplied_store,
        );
        let posted: Value =
            serde_json::from_str(&fixture.root.read("gh-fixture.posted")).assert_value();
        let body = posted["body"].as_str().assert_value();
        assert!(body.contains(&fixture._repository.main_revision));
        assert!(body.contains(&fixture._repository.feature_revision));
        capsule
            .cleanup
            .destroy_or_confirm_absent(RunRuntimeExit::Completed)
            .await
            .assert_value();

        fixture.run_id = RunId::new("resumed-hosted-comment");
        let successor = allocate_comment(&fixture).await;
        let resumed = dispatch_comment(&fixture, &successor, input, 3)
            .await
            .assert_value();
        assert_eq!(resumed, first);
        assert_eq!(store.load().await.assert_value(), Some(publication));
        assert_eq!(fixture.root.read("gh-fixture.posts"), "x");
        successor
            .cleanup
            .destroy_or_confirm_absent(RunRuntimeExit::Completed)
            .await
            .assert_value();
    }
}
