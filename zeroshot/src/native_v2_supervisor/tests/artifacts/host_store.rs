use super::*;
use crate::native_v2_files::FilePublicationOwner;
use crate::native_v2_files::publication::{FilePublication, ManagedPublicationStore};
use crate::native_v2_hosting::{ProductionHostingConfig, ProductionTargetControllerFactory};
use crate::v2_run_ledger::{CreateRun, RunLedger, sqlite::SqliteRunLedger};
use openengine_cluster_protocol::{FieldName, FileAddress, PublishedFile};
use std::{io, path::Path, sync::Mutex};

struct Capture {
    owner: FilePublicationOwner,
    paths: BTreeMap<FieldName, String>,
    files: PublishedFiles,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct HostStore {
    committed: Mutex<Option<Capture>>,
    unavailable: AtomicBool,
}

impl ManagedPublicationStore for HostStore {
    fn capture(
        &self,
        identity: &ConnectionIdentity,
        workspace: &Path,
        request: &FilePublication,
    ) -> io::Result<PublishedFiles> {
        assert_eq!(identity.principal().as_str(), "local");
        let (field, path) = request.paths.first_key_value().unwrap();
        let files = BTreeMap::from([(
            field.clone(),
            PublishedFile::Managed {
                target: "host-service".into(),
                address: FileAddress {
                    scope: FileScope::Run {
                        run_id: request.owner.run_id().clone(),
                    },
                    path: FilePath::new(path).unwrap(),
                },
            },
        )]);
        (request.preflight)(&files)?;
        let capture = Capture {
            owner: request.owner.clone(),
            paths: request.paths.clone(),
            files,
            bytes: std::fs::read(workspace.join(path))?,
        };
        assert!(self.committed.lock().unwrap().replace(capture).is_none());
        Err(io::Error::other(
            "simulated lost publication acknowledgement",
        ))
    }

    fn committed_receipt(
        &self,
        identity: &ConnectionIdentity,
        request: &FilePublication,
    ) -> io::Result<Option<PublishedFiles>> {
        assert_eq!(identity.principal().as_str(), "local");
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(io::Error::other("host store unavailable"));
        }
        let slot = self.committed.lock().unwrap();
        let capture = slot.as_ref().unwrap();
        assert_eq!(capture.owner, request.owner);
        assert_eq!(capture.paths, request.paths);
        Ok(Some(capture.files.clone()))
    }
}

#[tokio::test]
async fn custom_host_recovers_request_and_final_files_after_workspace_loss() {
    for is_request in [true, false] {
        let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let store = Arc::new(HostStore::default());
        let config = ProductionHostingConfig {
            storage_root: temp.path().to_owned(),
            file_store: Some((store.clone(), identity())),
            ..Default::default()
        };
        let mut breakpoint = if is_request { ask() } else { succeed("done") };
        breakpoint["artifacts"] = json!({"report":{"kind":"literal","path":"report.bin"}});
        let mut steps = vec![breakpoint];
        if is_request {
            steps.push(succeed("done"));
        }
        let mut harness = harness(
            graph(sequence(steps, state()), state()),
            json!({"text":"Review"}),
            FakeDriver::default(),
        )
        .await;
        if is_request {
            harness.supervisor =
                super::super::checkpoint_tests::RecordingCheckpointStore::attach(&harness).0;
        }
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let bytes = b"report\0\xff";
        std::fs::write(workspace.join("report.bin"), bytes).unwrap();
        let original = stored_run(&harness.ledger).await;
        let run_id = original.snapshot.run_id;
        let ledger = SqliteRunLedger::open(temp.path().join("runs.sqlite3")).unwrap();
        ledger
            .create_or_get(CreateRun {
                run_id: run_id.clone(),
                submission_key: original.submission_key,
                submission_digest: original.submission_digest,
                admitted: original.admitted,
            })
            .await
            .unwrap();
        let ledger = Arc::new(ledger);
        harness.supervisor.ledger = ledger.clone();
        harness.supervisor = harness
            .supervisor
            .with_file_publisher(config.file_publisher(&workspace));
        assert!(matches!(
            harness.supervisor.drive().await,
            Err(NativeV2SupervisorError::FilePublication(_))
        ));
        let interrupted = ledger.get(&run_id).await.unwrap().unwrap().snapshot;
        assert!(interrupted.primary_graph.is_none() && interrupted.suspension.is_none());
        assert!(interrupted.artifacts.is_empty());
        drop(harness);
        std::fs::remove_dir_all(workspace).unwrap();

        store.unavailable.store(true, Ordering::SeqCst);
        let factory = ProductionTargetControllerFactory::new(config);
        assert!(factory.create_controller().await.is_err());
        let pending = ledger.get(&run_id).await.unwrap().unwrap().snapshot;
        assert!(pending.terminal.is_none() && pending.suspension.is_none());
        store.unavailable.store(false, Ordering::SeqCst);
        let controller = factory.create_controller().await.unwrap();
        let recovered = ledger.get(&run_id).await.unwrap().unwrap().snapshot;
        let visible = if is_request {
            assert_eq!(recovered.phase, RunPhase::Suspended);
            controller
                .inbox_show(openengine_cluster_protocol::InboxShowParams::new(run_id))
                .await
                .unwrap()
                .request
                .artifacts
        } else {
            assert!(matches!(
                recovered.terminal,
                Some(TerminalResult::Succeeded { .. })
            ));
            recovered.artifacts
        };
        let committed = store.committed.lock().unwrap();
        let capture = committed.as_ref().unwrap();
        assert_eq!(visible, capture.files);
        assert_eq!(capture.bytes, bytes);
    }
}
