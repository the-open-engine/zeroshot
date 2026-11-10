use super::*;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use crate::native_v2_candidate::test_support::{
    allocation_request, full_graph, git_delivery_node, success_node,
};
use crate::native_v2_delivery::validation::TrustedDeliveryValidator;
use crate::native_v2_runner::NodeRunnerError;
use crate::native_v2_platform::CapsuleAllocationUnavailable;

#[derive(Default)]
struct Factory {
    seen: Mutex<Vec<Observed>>,
    wrong_workspace: bool,
}

struct Observed {
    run_id: String,
    delivery_id: String,
    revision: String,
    workspace: PathBuf,
    writer_uid: u32,
}

#[async_trait]
impl HostedDeliveryValidationFactory for Factory {
    async fn prepare(
        &self,
        request: HostedDeliveryValidationRequest<'_>,
    ) -> Result<Option<TrustedDeliveryValidator>, NodeRunnerError> {
        self.seen.lock().unwrap().push(Observed {
            run_id: request.run_id.as_str().to_owned(),
            delivery_id: request.delivery_run_id.as_str().to_owned(),
            revision: request
                .admitted
                .source
                .as_ref()
                .unwrap()
                .revision
                .as_str()
                .to_owned(),
            workspace: request.workspace.to_owned(),
            writer_uid: request
                .process_pool
                .identity(crate::execution::process::HostedProcessScope::Delivery)
                .unwrap()
                .uid(),
        });
        let executable = request.runtime_home.join("validator");
        openengine_cluster_testkit::fixture::write_executable(
            &executable,
            "#!/bin/sh\nprintf '{\"verdict\":\"accepted\"}'\n",
            0o700,
        )
        .unwrap();
        let other = request.runtime_home.join("other-workspace");
        let workspace = if self.wrong_workspace {
            fs::create_dir(&other).unwrap();
            other.as_path()
        } else {
            request.workspace
        };
        Ok(Some(
            TrustedDeliveryValidator::new(executable, workspace).unwrap(),
        ))
    }
}

async fn admitted_delivery(revision: &str) -> native_v2_contract::AdmittedRun {
    let mut request = submission(runtime(BTreeSet::new()), revision, "host-validation");
    request.graph = full_graph(vec![git_delivery_node(), success_node()]);
    request.runtime = serde_json::from_value(json!({
        "harness":"codex", "provider":"openai", "size":"medium", "nodes":{
            "deliver":{"kind":"git_delivery","connections":{}}
        },
        "activeMemory":{"repository":false,"graph":false},
        "globalAgents":{"postmortem":{"enabled":false}}
    }))
    .unwrap();
    crate::native_v2_admission::NativeV2Admission
        .admit(request)
        .await
        .unwrap()
}

fn configured_allocator(
    root: &TestDirectory,
    repository: &RepositoryFixture,
    factory: Arc<Factory>,
) -> ProductionCapsuleAllocator {
    prepare_storage_root(&root.path().to_owned()).unwrap();
    let mut config = capsule_config(root.path().to_owned());
    config.delivery_validation = Some(factory);
    capsule_allocator(config)
        .unwrap()
        .with_test_filesystem_and_source(repository.remote.clone(), portable_filesystem)
}

#[tokio::test]
async fn factory_receives_exact_allocated_identity_only_for_delivery_graphs() {
    for delivery in [false, true] {
        let repository = RepositoryFixture::new();
        let root = TestDirectory::new("host-validation-factory");
        let factory = Arc::new(Factory::default());
        let allocator = configured_allocator(&root, &repository, factory.clone());
        let admitted = if delivery {
            admitted_delivery(&repository.main_revision).await
        } else {
            admit(submission(
                runtime(BTreeSet::new()),
                &repository.main_revision,
                "no-host-validation",
            ))
            .await
        };
        let run = RunId::new("host-validation-factory");
        let capsule = allocator
            .allocate(allocation_request(&run, &admitted, Some("private-token")))
            .await
            .unwrap();
        {
            let seen = factory.seen.lock().unwrap();
            assert_eq!(seen.len(), usize::from(delivery));
            if let Some(observed) = seen.first() {
                assert_eq!(observed.run_id, run.as_str());
                assert_eq!(observed.delivery_id, run.as_str());
                assert_eq!(observed.revision, repository.main_revision);
                assert_eq!(
                    observed.workspace,
                    allocator.run_path(&run).join("workspace")
                );
                assert_ne!(observed.writer_uid, 0);
            }
        }
        capsule
            .checkpoints
            .as_ref()
            .unwrap()
            .finish(&[])
            .await
            .unwrap();
        capsule
            .cleanup
            .destroy_or_confirm_absent(RunRuntimeExit::Completed)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_factory_cannot_bind_another_workspace_to_the_candidate() {
    let repository = RepositoryFixture::new();
    let root = TestDirectory::new("host-validation-wrong-workspace");
    let factory = Arc::new(Factory {
        wrong_workspace: true,
        ..Default::default()
    });
    let allocator = configured_allocator(&root, &repository, factory.clone());
    let admitted = admitted_delivery(&repository.main_revision).await;
    let run = RunId::new("host-validation-wrong-workspace");
    let failure = allocator
        .allocate(allocation_request(&run, &admitted, Some("private-token")))
        .await
        .err()
        .unwrap();
    assert!(matches!(failure, CapsuleAllocationUnavailable::Runtime));
    assert_eq!(factory.seen.lock().unwrap().len(), 1);
    assert!(!allocator.run_path(&run).exists());
}
