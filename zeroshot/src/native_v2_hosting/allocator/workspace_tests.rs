use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_files::{inputs::InputAdmission, FileAccess, FilePin, WorkspaceFiles};
use openengine_cluster_protocol::{IdempotencyKey, PayloadType, RunFileInput};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

fn allocator(root: &Path, files: Option<WorkspaceFiles>) -> ProductionCapsuleAllocator {
    let mut config = crate::native_v2_hosting::tests::fixtures::capsule_config(root.to_owned());
    config.workspace_files = files;
    crate::native_v2_hosting::tests::fixtures::capsule_allocator(config).assert_value()
}

#[test]
fn request_file_admission_requires_storage_but_scalar_answers_do_not() {
    let root = TestDirectory::new("host-file-admission-disabled");
    let allocator = allocator(root.path(), None);
    let run = RunId::new("successor");
    let key = IdempotencyKey::new("answer").assert_value();
    let value = json!("caller value");
    let admission = |schema| InputAdmission {
        run_id: &run,
        owner: &run,
        key: &key,
        schema,
        value: &value,
        local_root: None,
    };
    let scalar = allocator
        .accept_request_files(admission(&PayloadType::String))
        .assert_value();
    assert_eq!(scalar.value, value);
    assert!(scalar.claim.is_none());
    assert!(
        allocator
            .accept_request_files(admission(&PayloadType::Filepath))
            .unwrap_err()
            .contains("does not enable file inputs")
    );
}

#[test]
fn answer_upload_release_preserves_the_accepted_lineage_until_its_claim_is_refused() {
    let root = TestDirectory::new("host-answer-lineage");
    let files = crate::native_v2_cli::files::workspace_files(&root.child("library")).assert_value();
    let allocator = allocator(&root.child("host"), Some(files.clone()));
    let predecessor = RunId::new("predecessor");
    let origin = RunId::new("lineage-owner");
    write_recovery(
        &recovery_path(&allocator.config.storage_root, &predecessor),
        &HostedRecoveryDocument {
            recoverable: true,
            run_id: Some(predecessor.clone()),
            delivery_run_id: Some(origin.clone()),
            resumed_from: None,
            successor_run_id: None,
        },
    )
    .assert_value();
    let original = root.write("source.csv", "frozen answer bytes");
    let pin = FilePin {
        run_id: RunId::new("upload"),
        operation: IdempotencyKey::new("answer-upload").assert_value(),
    };
    let mut bytes = Vec::new();
    crate::native_v2_files::pack(&original, &mut bytes).assert_value();
    let access = FileAccess {
        target: &files.target,
        identity: &files.identity,
    };
    files
        .host
        .stage_upload(&access, &pin, &mut std::io::Cursor::new(bytes))
        .assert_value();
    let value = serde_json::to_value(RunFileInput::Staged {
        run_id: pin.run_id.clone(),
        operation: pin.operation.clone(),
    })
    .assert_value();
    let frozen = allocator
        .accept_request_files(InputAdmission {
            run_id: &RunId::new("successor"),
            owner: &predecessor,
            key: &IdempotencyKey::new("answer-key").assert_value(),
            schema: &PayloadType::Filepath,
            value: &value,
            local_root: None,
        })
        .assert_value();
    let accepted = PathBuf::from(frozen.value.as_str().assert_value());
    allocator.release_request_uploads(&PayloadType::Filepath, &value);
    assert!(files.host.read_pin(&access, &pin, &mut Vec::new()).is_err());
    assert!(
        files
            .file_input_roots(&predecessor)
            .assert_value()
            .is_empty()
    );
    assert_eq!(
        files
            .materialize_file_inputs(&origin, true)
            .assert_value()
            .len(),
        1
    );
    assert_eq!(
        std::fs::read(&accepted).assert_value(),
        b"frozen answer bytes"
    );
    allocator.release_refused_answer(&frozen.claim.assert_value());
    assert!(files.file_input_roots(&origin).assert_value().is_empty());
    assert!(!accepted.exists());
    assert_eq!(
        std::fs::read(original).assert_value(),
        b"frozen answer bytes"
    );
}

#[tokio::test]
async fn retained_input_cleanup_follows_recovery_eligibility_and_final_disposal() {
    for discard in [false, true] {
        let root = TestDirectory::new("host-retained-input-lifecycle");
        let files =
            crate::native_v2_cli::files::workspace_files(&root.child("library")).assert_value();
        let allocator = allocator(&root.child("host"), Some(files.clone()));
        let run = RunId::new("retained-part");
        let origin = RunId::new("lineage-owner");
        let run_root = run_directory(&allocator.config.storage_root, &run);
        std::fs::create_dir_all(run_root.join("workspace")).assert_value();
        std::fs::create_dir(run_root.join("runtime")).assert_value();
        std::fs::write(run_root.join("workspace/output"), "retained edit").assert_value();
        let recovery = recovery_path(&allocator.config.storage_root, &run);
        write_recovery(
            &recovery,
            &HostedRecoveryDocument {
                recoverable: true,
                run_id: Some(run.clone()),
                delivery_run_id: Some(origin.clone()),
                resumed_from: None,
                successor_run_id: None,
            },
        )
        .assert_value();
        let original = root.write("source.csv", "accepted original");
        let frozen = files
            .freeze_file_inputs(InputAdmission {
                run_id: &run,
                owner: &origin,
                key: &IdempotencyKey::new("input-key").assert_value(),
                schema: &PayloadType::Filepath,
                value: &json!(original),
                local_root: Some(root.path()),
            })
            .assert_value();
        let accepted = PathBuf::from(frozen.value.as_str().assert_value());
        files.materialize_file_inputs(&origin, true).assert_value();
        allocator
            .release_disposed_inputs(&run, &origin, RunRuntimeExit::Failed)
            .await
            .assert_value();
        assert_eq!(
            std::fs::read(&accepted).assert_value(),
            b"accepted original"
        );
        if discard {
            assert!(
                !allocator
                    .discard_retained_workspace(&RunId::new("unknown"))
                    .await
                    .assert_value()
            );
            assert!(
                allocator
                    .discard_retained_workspace(&run)
                    .await
                    .assert_value()
            );
            assert!(!run_root.exists());
            assert!(!read_recovery(&recovery).assert_value().recoverable);
            assert!(
                !allocator
                    .discard_retained_workspace(&run)
                    .await
                    .assert_value()
            );
            allocator
                .release_disposed_inputs(&run, &origin, RunRuntimeExit::Failed)
                .await
                .assert_value();
        } else {
            allocator
                .release_disposed_inputs(&run, &origin, RunRuntimeExit::Completed)
                .await
                .assert_value();
            assert_eq!(
                std::fs::read(run_root.join("workspace/output")).assert_value(),
                b"retained edit"
            );
        }
        assert!(!accepted.exists());
        assert!(files.file_input_roots(&origin).assert_value().is_empty());
        assert_eq!(std::fs::read(original).assert_value(), b"accepted original");
    }
}

#[tokio::test]
async fn completed_input_cleanup_preserves_an_explicit_managed_publication() {
    use crate::native_v2_files::{publication::FilePublication, FilePublicationOwner, TargetFileHost};
    use openengine_cluster_protocol::{
        FieldName, FilePath, FileSelection, PublishedFile, UnixTimestampMillis,
    };
    use std::io::Seek as _;

    let root = TestDirectory::new("host-input-and-publication-retention");
    let state = root.child("library");
    let files = crate::native_v2_cli::files::workspace_files(&state).assert_value();
    let (store, identity, _) =
        crate::native_v2_cli::files::local_file_store_at(&state).assert_value();
    let store = Arc::new(store);
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    assert!(
        allocator(&root.child("disabled"), None)
            .config
            .file_publisher(&workspace)
            .is_none()
    );
    let mut config = crate::native_v2_hosting::tests::fixtures::capsule_config(root.child("host"));
    config.workspace_files = Some(files.clone());
    config.file_store = Some((store.clone(), identity.clone()));
    let allocator =
        crate::native_v2_hosting::tests::fixtures::capsule_allocator(config).assert_value();
    let run = RunId::new("completed-publication");
    let original = root.write("original.csv", "accepted input");
    let frozen = files
        .freeze_file_inputs(InputAdmission {
            run_id: &run,
            owner: &run,
            key: &IdempotencyKey::new("input").assert_value(),
            schema: &PayloadType::Filepath,
            value: &json!(original),
            local_root: Some(root.path()),
        })
        .assert_value();
    let accepted = PathBuf::from(frozen.value.as_str().assert_value());
    files.materialize_file_inputs(&run, true).assert_value();
    let output = workspace.join("report.csv");
    std::fs::write(&output, "published output").assert_value();
    let (_stop, stopping) = tokio::sync::watch::channel(false);
    let published = allocator
        .config
        .file_publisher(&workspace)
        .assert_value()
        .publish(FilePublication {
            owner: FilePublicationOwner::Final {
                run_id: run.clone(),
            },
            paths: BTreeMap::from([(FieldName::new("report").assert_value(), "report.csv".into())]),
            now: UnixTimestampMillis::new(1).assert_value(),
            cancellation: crate::execution::driver::DriverCancellation::new(stopping),
            preflight: Arc::new(|files| {
                assert!(matches!(
                    files.values().next(),
                    Some(PublishedFile::Managed { .. })
                ));
                Ok(())
            }),
        })
        .await
        .assert_value();
    std::fs::write(&output, "later workspace edit").assert_value();
    allocator
        .release_disposed_inputs(&run, &run, RunRuntimeExit::Completed)
        .await
        .assert_value();
    assert!(!accepted.exists());
    assert!(files.file_input_roots(&run).assert_value().is_empty());
    let PublishedFile::Managed { address, .. } = published.values().next().assert_value() else {
        panic!("expected managed publication");
    };
    let mut packed = tempfile::tempfile().assert_value();
    store
        .download(
            &FileAccess {
                target: store.target(),
                identity: &identity,
            },
            &FileSelection {
                address: address.clone(),
                member: FilePath::new("").assert_value(),
            },
            &mut packed,
        )
        .assert_value();
    packed.rewind().assert_value();
    let restored = root.child("downloaded-report");
    crate::native_v2_files::unpack(&mut packed, &restored).assert_value();
    assert_eq!(std::fs::read(restored).assert_value(), b"published output");
    assert_eq!(
        std::fs::read(output).assert_value(),
        b"later workspace edit"
    );
    assert_eq!(std::fs::read(original).assert_value(), b"accepted input");
}
