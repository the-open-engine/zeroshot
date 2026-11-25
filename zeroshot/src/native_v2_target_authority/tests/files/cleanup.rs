use super::*;
use crate::native_v2_files::{
    FileAccess, FilePublicationOwner, TargetFileHost, publication::FilePublication,
};
use crate::native_v2_platform::{CapsuleAllocationRequest, NativeV2PlatformController};
use crate::v2_run_ledger::{CreateRun, RunEvent, RunLedger};
use openengine_cluster_protocol::{PublishedFiles, TerminalResult, UnixTimestampMillis};
use tokio::sync::Notify;

#[derive(Default)]
struct CleanupGate {
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl CapsuleAllocator for CleanupGate {
    async fn claim_controller(
        &self,
        run: &RunId,
    ) -> Result<Arc<dyn ExclusiveControllerClaim>, ControllerClaimUnavailable> {
        NoAllocation.claim_controller(run).await
    }
    async fn allocate(
        &self,
        _: CapsuleAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        panic!("cleanup must not allocate")
    }
    async fn destroy_or_confirm_absent(
        &self,
        run: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        NoAllocation.destroy_or_confirm_absent(run, exit).await
    }
    async fn discard_workspace(&self, _: &RunId) -> Result<bool, CapsuleCleanupUnavailable> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(false)
    }
}

struct CleanupFactory(Arc<NativeV2PlatformController>);
#[async_trait]
impl TargetControllerFactory for CleanupFactory {
    fn run_sizes(&self) -> Option<openengine_cluster_protocol::TargetRunSizesDiscovery> {
        Some(fixture_run_sizes())
    }
    async fn create(&self) -> Result<Arc<NativeV2PlatformController>, TargetAuthorityError> {
        Ok(self.0.clone())
    }
    async fn submit(
        &self,
        _: &NativeV2PlatformController,
        _: TargetRunRequest,
    ) -> Result<TargetRunReceipt, TargetAuthorityError> {
        panic!("fixture submits directly to its ledger")
    }
}

fn capture(
    store: &DiskFileStore,
    root: &std::path::Path,
    owner: FilePublicationOwner,
) -> PublishedFiles {
    let (_, signal) = tokio::sync::watch::channel(false);
    store
        .capture_publication(
            &FileAccess {
                target: "selected",
                identity: &identity(),
            },
            root,
            &FilePublication {
                owner,
                paths: [("report".parse().unwrap(), "report".to_owned())]
                    .into_iter()
                    .collect(),
                now: UnixTimestampMillis::new(1).unwrap(),
                cancellation: crate::execution::driver::DriverCancellation::new(signal),
                preflight: Arc::new(|_| Ok(())),
            },
        )
        .unwrap()
}

async fn pending(ledger: &FakeRunLedger, store: &DiskFileStore, root: &std::path::Path) -> RunId {
    let run_id = super::super::requests::seed(ledger, 9, false).await;
    capture(
        store,
        root,
        FilePublicationOwner::Request {
            run_id: run_id.clone(),
            request: IdempotencyKey::new("question-9").unwrap(),
        },
    );
    run_id
}

#[tokio::test]
async fn cleanup_http_freezes_selection_before_pending_request_publication() {
    for late in [false, true] {
        let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        std::fs::create_dir(temp.path().join("report")).unwrap();
        std::fs::write(temp.path().join("report/proof"), "pending bytes").unwrap();
        let ledger = Arc::new(FakeRunLedger::new());
        let gate = Arc::new(CleanupGate::default());
        let controller = Arc::new(
            NativeV2PlatformController::new(ledger.clone(), gate.clone())
                .await
                .unwrap(),
        );
        let old = RunId::new("old-terminal");
        ledger
            .create_or_get(CreateRun {
                run_id: old.clone(),
                submission_key: IdempotencyKey::new("old").unwrap(),
                submission_digest: openengine_cluster_protocol::Sha256Digest::new("a".repeat(64))
                    .unwrap(),
                admitted: crate::native_v2_runner::test_support::admitted(),
            })
            .await
            .unwrap();
        ledger
            .append(
                &old,
                vec![
                    RunEvent::RunStarted,
                    RunEvent::Terminal {
                        result: TerminalResult::Failed {
                            reason: "worker_failed".parse().unwrap(),
                            context: None,
                        },
                    },
                ],
            )
            .await
            .unwrap();
        let store = Arc::new(
            DiskFileStore::open(
                temp.path().join("files"),
                "selected".to_owned(),
                Arc::new(SingleAccountFiles {
                    principal: "test-user".to_owned(),
                    tenant: "test-target".to_owned(),
                }),
            )
            .unwrap(),
        );
        capture(
            &store,
            temp.path(),
            FilePublicationOwner::Final {
                run_id: old.clone(),
            },
        );
        if !late {
            pending(&ledger, &store, temp.path()).await;
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let target = Arc::new(NativeV2TargetAuthority::new(Arc::new(CleanupFactory(
            controller.clone(),
        ))));
        let server = NativeV2TargetServer::new_direct(
            target,
            identity(),
            format!("ws://{address}/native-v2/oecp"),
        )
        .unwrap()
        .with_files(TargetFilesMount {
            target: "selected".to_owned(),
            host: store.clone(),
            transfers: temp.path().join("transfers"),
        })
        .unwrap();
        let task = tokio::spawn(Arc::new(server).serve(listener));
        let cleanup = tokio::spawn(async move {
            let metadata = serde_json::to_vec(&FileRequest::Cleanup { all: true }).unwrap();
            let mut body = (metadata.len() as u32).to_be_bytes().to_vec();
            body.extend(metadata);
            let response = reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .post(format!("http://{address}/native-v2/files"))
                .header("Content-Type", TARGET_FILES_MEDIA_TYPE)
                .body(body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::OK);
            response.json::<Value>().await.unwrap()
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), gate.entered.notified())
            .await
            .unwrap();
        let run_id = if late {
            pending(&ledger, &store, temp.path()).await
        } else {
            RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c909")
        };
        assert!(
            controller
                .protected_file_runs()
                .await
                .unwrap()
                .contains(&run_id)
        );
        gate.release.notify_one();
        assert_eq!(
            cleanup.await.unwrap(),
            json!({"kind":"cleaned","publications":1,"checkpointLineages":0})
        );
        let access = FileAccess {
            target: "selected",
            identity: &identity(),
        };
        assert_eq!(
            store
                .list(
                    &access,
                    FileScope::Run {
                        run_id: run_id.clone()
                    },
                    &FilePath::new("").unwrap()
                )
                .unwrap()
                .len(),
            1,
            "late={late}"
        );
        assert!(
            store
                .list(
                    &access,
                    FileScope::Run { run_id: old },
                    &FilePath::new("").unwrap()
                )
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            ledger.get(&run_id).await.unwrap().unwrap().snapshot.phase,
            crate::v2_run_ledger::RunPhase::Suspended
        );
        task.abort();
    }
}
