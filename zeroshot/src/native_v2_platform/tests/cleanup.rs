use super::*;
use openengine_cluster_protocol::{Sha256Digest, UnixTimestampMillis};
use crate::v2_run_ledger::{CreateRun, RunEvent, RunLedger};

struct RetentionAllocator {
    runtime: Arc<FakeAllocator>,
    retained: StdMutex<BTreeSet<RunId>>,
}
#[async_trait]
impl CapsuleAllocator for RetentionAllocator {
    async fn claim_controller(
        &self,
        run: &RunId,
    ) -> Result<Arc<dyn ExclusiveControllerClaim>, ControllerClaimUnavailable> {
        self.runtime.claim_controller(run).await
    }
    async fn allocate(
        &self,
        request: CapsuleAllocationRequest<'_>,
    ) -> Result<AllocatedCapsule, CapsuleAllocationUnavailable> {
        self.runtime.allocate(request).await
    }
    async fn destroy_or_confirm_absent(
        &self,
        run: &RunId,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        self.runtime.destroy_or_confirm_absent(run, exit).await
    }
    async fn discard_workspace(&self, run: &RunId) -> Result<bool, CapsuleCleanupUnavailable> {
        Ok(self.retained.lock().unwrap().remove(run))
    }
}
#[tokio::test]
async fn cleanup_uses_latest_terminal_time_and_preserves_queued_and_active_runs() {
    use std::sync::atomic::AtomicU64;
    const DAYS_60: u64 = 60 * 24 * 60 * 60 * 1000;
    let clock = Arc::new(AtomicU64::new(1000));
    let read_clock = clock.clone();
    let ledger = Arc::new(FakeRunLedger::new().with_wall_clock(Arc::new(move || {
        UnixTimestampMillis::new(read_clock.load(Ordering::SeqCst)).unwrap()
    })));
    let driver = Arc::new(FakeDriver::new(Behavior::Hang));
    let cleanup = Arc::new(FakeCleanup::new(ledger.clone()));
    let allocator = Arc::new(RetentionAllocator {
        runtime: Arc::new(FakeAllocator::new(driver, cleanup)),
        retained: StdMutex::default(),
    });
    let controller = NativeV2PlatformController::new(ledger.clone(), allocator.clone())
        .await
        .unwrap();
    let admitted = crate::native_v2_runner::test_support::admitted();
    for name in ["old", "recent", "queued", "active"] {
        let id = RunId::new(name);
        ledger
            .create_or_get(CreateRun {
                run_id: id.clone(),
                submission_key: IdempotencyKey::new(name).unwrap(),
                submission_digest: Sha256Digest::new("a".repeat(64)).unwrap(),
                admitted: admitted.clone(),
            })
            .await
            .unwrap();
        allocator.retained.lock().unwrap().insert(id.clone());
        if name != "queued" {
            ledger
                .append(&id, vec![RunEvent::RunStarted])
                .await
                .unwrap();
        }
        if name == "old" || name == "recent" {
            ledger
                .append(
                    &id,
                    vec![RunEvent::Terminal {
                        result: TerminalResult::Failed {
                            reason: EnumLabel::new("worker_failed").unwrap(),
                            context: None,
                        },
                    }],
                )
                .await
                .unwrap();
            clock.fetch_add(1, Ordering::SeqCst);
        }
    }
    let now = UnixTimestampMillis::new(1000 + DAYS_60).unwrap();
    assert_eq!(controller.cleanup_checkpoints(false, now).await.unwrap(), 1);
    assert_eq!(controller.cleanup_checkpoints(false, now).await.unwrap(), 0);
    assert_eq!(controller.cleanup_checkpoints(true, now).await.unwrap(), 1);
    assert_eq!(
        *allocator.retained.lock().unwrap(),
        [RunId::new("active"), RunId::new("queued")]
            .into_iter()
            .collect()
    );
    assert_eq!(
        controller.protected_file_runs().await.unwrap(),
        *allocator.retained.lock().unwrap()
    );
    assert_eq!(ledger.list().await.unwrap().len(), 4);
}
