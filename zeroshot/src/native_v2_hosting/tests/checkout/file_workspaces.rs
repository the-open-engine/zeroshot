#![cfg(target_os = "linux")]
use super::*;
use openengine_cluster_protocol::{
    FileAddress, FilePath, FileScope, FileSelection, IdempotencyKey, RunWorkspace,
    WorkspacePlacement,
};
use crate::native_v2_files::{FileAccess, FileReplaceRequest, FreezeWorkspace, WorkspaceFiles};

async fn fixture(storage: Option<Arc<dyn HostedWorkspaceStorage>>) -> CheckoutFixture {
    let mut fixture = CheckoutFixture::with_workspace_storage("exit 97", storage).await;
    fixture.admitted.source = None;
    fixture.admitted.workspace = Some(RunWorkspace {
        placement: WorkspacePlacement::Managed,
        local_directory: None,
        seeds: vec![],
        copy_root: false,
    });
    fixture
}

fn rebuild(fixture: &mut CheckoutFixture) {
    fixture.allocator =
        ProductionCapsuleAllocator::new(fixture.config.clone(), fixture.diagnostics.clone())
            .assert_value()
            .with_test_filesystem_and_source(
                fixture._repository.remote.clone(),
                portable_filesystem,
            );
}

fn upload(files: &WorkspaceFiles, selection: &FileSelection, source: &Path, operation: &str) {
    let mut bytes = Vec::new();
    crate::native_v2_files::pack(source, &mut bytes).assert_value();
    files
        .host
        .replace(
            &FileAccess {
                target: &files.target,
                identity: &files.identity,
            },
            &FileReplaceRequest {
                address: selection.address.clone(),
                operation: IdempotencyKey::new(operation).assert_value(),
            },
            &mut std::io::Cursor::new(bytes),
        )
        .assert_value();
}

#[tokio::test]
async fn direct_workspace_seeds_precede_startup_and_selected_checkpoint_survives_allocator_restart()
{
    for exit in [RunRuntimeExit::Completed, RunRuntimeExit::ForceStopped] {
        exercise_workspace_recovery(false, exit).await;
    }
}

#[tokio::test]
async fn root_direct_workspace_startup_observes_seeds_before_selected_checkpoint_recovery() {
    if unsafe { libc::geteuid() } == 0 {
        exercise_workspace_recovery(true, RunRuntimeExit::Completed).await;
    }
}

async fn exercise_workspace_recovery(startup: bool, exit: RunRuntimeExit) {
    let mut fixture = fixture(None).await;
    let files = crate::native_v2_cli::files::workspace_files(&fixture.root.child("library-state"))
        .assert_value();
    let source = fixture.root.child("source-folder");
    fs::create_dir_all(source.join("empty")).assert_value();
    fs::write(source.join("data"), b"original seed").assert_value();
    let selection = FileSelection {
        address: FileAddress {
            scope: FileScope::User,
            path: FilePath::new("library/nested").assert_value(),
        },
        member: FilePath::new("").assert_value(),
    };
    upload(&files, &selection, &source, "first");
    fixture
        .admitted
        .workspace
        .as_mut()
        .unwrap()
        .seeds
        .push(selection.clone());
    files
        .freeze(FreezeWorkspace {
            run_id: &fixture.run_id,
            submission_key: &IdempotencyKey::new("workspace-admission").assert_value(),
            workspace: fixture.admitted.workspace.as_ref().unwrap(),
            local_root: None,
        })
        .assert_value();
    fixture.admitted.environment = Some(
        serde_json::from_value(if startup {
            json!({"startup":"test ! -d .git; test -d nested/empty; pwd > startup-path"})
        } else {
            json!({})
        })
        .assert_value(),
    );
    Arc::make_mut(&mut fixture.config).workspace_files = Some(files.clone());
    rebuild(&mut fixture);
    let capsule = fixture
        .allocator
        .allocate(
            crate::native_v2_candidate::test_support::allocation_request(
                &fixture.run_id,
                &fixture.admitted,
                None,
            ),
        )
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error:?}: {:?}",
                fixture.diagnostics.snapshot(&fixture.run_id)
            )
        });
    let workspace = fixture
        .allocator
        .run_path(&fixture.run_id)
        .join("workspace");
    assert_eq!(
        fs::read(workspace.join("nested/data")).assert_value(),
        b"original seed"
    );
    if startup {
        assert_eq!(
            fs::read_to_string(workspace.join("startup-path"))
                .assert_value()
                .trim(),
            workspace.to_str().unwrap()
        );
    }
    assert!(!workspace.join(".git").exists());
    fs::write(workspace.join("nested/data"), b"checkpoint edit").assert_value();
    capture_checkpoint(&capsule).await;
    let selected = fixture.checkpoints(&fixture.run_id).await.checkpoints[0]
        .checkpoint_id
        .clone();
    fs::write(workspace.join("nested/data"), b"later run edit").assert_value();
    fs::write(source.join("data"), b"new library bytes").assert_value();
    upload(&files, &selection, &source, "replacement");
    fs::remove_dir_all(&source).assert_value();
    finish_checkpoints(&capsule).await;
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    rebuild(&mut fixture);
    let successor = RunId::new("workspace-successor");
    let restored = fixture
        .allocator
        .allocate_from_retained(fixture.retained_request(
            &fixture.run_id,
            &successor,
            CheckpointRestoreSelection::Checkpoint {
                checkpoint_id: selected,
            },
        ))
        .await
        .assert_value();
    let workspace = fixture.allocator.run_path(&successor).join("workspace");
    assert_eq!(
        fs::read(workspace.join("nested/data")).assert_value(),
        b"checkpoint edit"
    );
    assert!(!workspace.join(".git").exists());
    assert_eq!(
        fs::read(fixture.root.child("git-script.attempts")).assert_value(),
        b""
    );
    restored
        .cleanup
        .destroy_or_confirm_absent(exit)
        .await
        .assert_value();
    let destination = fixture.root.child("released-input-probe");
    fs::create_dir(&destination).assert_value();
    assert!(files.materialize(&fixture.run_id, &destination).is_err());
    files
        .host
        .remove(
            &FileAccess {
                target: &files.target,
                identity: &files.identity,
            },
            &selection.address,
        )
        .assert_value();
    assert_eq!(
        fs::read_dir(fixture.root.child("library-state/files/objects"))
            .assert_value()
            .count(),
        0
    );
}

struct SourceFreeStorage;

#[async_trait::async_trait]
impl HostedWorkspaceStorage for SourceFreeStorage {
    async fn retain_delivery_state(
        &self,
        _: &RunId,
        _: &crate::native_v2_contract::DeliveryCheckpoint,
    ) -> Result<(), crate::native_v2_supervisor::checkpoints::CheckpointError> {
        Ok(())
    }
    async fn prepare(
        &self,
        request: HostedWorkspaceRequest<'_>,
    ) -> Result<HostedWorkspace, crate::native_v2_supervisor::checkpoints::CheckpointError> {
        assert!(!request.workspace.join(".git").exists());
        assert!(!request.workspace_checkpoints);
        fs::write(
            request.workspace.join("adapter-file"),
            b"hosted adapter without source",
        )?;
        Ok(HostedWorkspace {
            resumed_from: None,
            vector_seed: vec![],
            comment_publications: None,
            checkpoints: Arc::new(
                crate::native_v2_supervisor::checkpoints::DeliveryCheckpointStore::new(
                    request.checkpoint_directory.join("metadata"),
                ),
            ),
            execution_seed: vec![],
            request_continuation: None,
            delivery_run_id: None,
        })
    }
}

#[tokio::test]
async fn hosted_workspace_adapter_and_empty_environment_need_no_source() {
    let mut fixture = fixture(Some(Arc::new(SourceFreeStorage))).await;
    fixture.admitted.runtime.workspace_checkpoints = false;
    fixture.admitted.environment = Some(serde_json::from_value(json!({})).assert_value());
    Arc::make_mut(&mut fixture.config).workspace_checkpoint_opt_out = true;
    rebuild(&mut fixture);
    let capsule = fixture
        .allocator
        .allocate(
            crate::native_v2_candidate::test_support::allocation_request(
                &fixture.run_id,
                &fixture.admitted,
                None,
            ),
        )
        .await
        .assert_value();
    let workspace = fixture
        .allocator
        .run_path(&fixture.run_id)
        .join("workspace");
    assert_eq!(
        fs::read(workspace.join("adapter-file")).assert_value(),
        b"hosted adapter without source"
    );
    assert_eq!(
        fs::read(fixture.root.child("git-script.attempts")).assert_value(),
        b""
    );
    assert!(
        fixture
            .checkpoints(&fixture.run_id)
            .await
            .checkpoints
            .is_empty()
    );
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Completed)
        .await
        .assert_value();
}
