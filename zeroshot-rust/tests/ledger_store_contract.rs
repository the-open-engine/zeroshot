mod support;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use support::ledger::{run_store_contract, ManualClock};
use tempfile::tempdir;
use zeroshot_engine::ledger::{
    Clock, LedgerStore, MemoryLedgerStore, OwnerId, ResourceId, SqliteLedgerStore, StoreError,
};

struct BlockingClock {
    now: AtomicU64,
    reads: AtomicU64,
    block_next: AtomicBool,
    entered: (Mutex<bool>, Condvar),
    released: (Mutex<bool>, Condvar),
}

impl BlockingClock {
    fn at(now: u64) -> Self {
        Self {
            now: AtomicU64::new(now),
            reads: AtomicU64::new(0),
            block_next: AtomicBool::new(false),
            entered: (Mutex::new(false), Condvar::new()),
            released: (Mutex::new(false), Condvar::new()),
        }
    }

    fn block_next_read(&self) {
        self.block_next.store(true, Ordering::SeqCst);
    }

    fn wait_until_blocked(&self) {
        let entered = self.entered.0.lock().unwrap();
        let _guard = self
            .entered
            .1
            .wait_while(entered, |entered| !*entered)
            .unwrap();
    }

    fn wait_for_read_after(&self, count: u64, timeout: Duration) -> bool {
        let guard = self.entered.0.lock().unwrap();
        let (_guard, _) = self
            .entered
            .1
            .wait_timeout_while(guard, timeout, |_| {
                self.reads.load(Ordering::SeqCst) <= count
            })
            .unwrap();
        self.reads.load(Ordering::SeqCst) > count
    }

    fn advance(&self, millis: u64) {
        self.now.fetch_add(millis, Ordering::SeqCst);
    }

    fn release(&self) {
        *self.released.0.lock().unwrap() = true;
        self.released.1.notify_all();
    }
}

impl Clock for BlockingClock {
    fn now_unix_millis(&self) -> u64 {
        let sampled = self.now.load(Ordering::SeqCst);
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.entered.1.notify_all();
        if self.block_next.swap(false, Ordering::SeqCst) {
            *self.entered.0.lock().unwrap() = true;
            self.entered.1.notify_all();
            let released = self.released.0.lock().unwrap();
            let _guard = self
                .released
                .1
                .wait_while(released, |released| !*released)
                .unwrap();
        }
        sampled
    }
}

#[tokio::test]
async fn deterministic_memory_store_passes_shared_contract() {
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(MemoryLedgerStore::new(clock.clone()));
    run_store_contract(store, clock).await;
}

#[tokio::test]
async fn sqlite_store_passes_shared_contract_unchanged() {
    let directory = tempdir().unwrap();
    let clock = Arc::new(ManualClock::at(100));
    let store = Arc::new(SqliteLedgerStore::new(directory.path(), clock.clone()).unwrap());
    run_store_contract(store, clock).await;
}

#[tokio::test]
async fn memory_fence_time_is_sampled_after_the_resource_mutation_lock() {
    let clock = Arc::new(BlockingClock::at(100));
    let store = Arc::new(MemoryLedgerStore::new(clock.clone()));
    let resource = ResourceId::new("memory-fence-clock").unwrap();
    store.create_resource(&resource).await.unwrap();
    let fence = store
        .acquire_fence(&resource, &OwnerId::new("owner").unwrap(), 10)
        .await
        .unwrap();

    clock.block_next_read();
    let validation = {
        let store = store.clone();
        let resource = resource.clone();
        let fence = fence.clone();
        tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(store.validate_fence(&resource, &fence))
        })
    };
    {
        let clock = clock.clone();
        tokio::task::spawn_blocking(move || clock.wait_until_blocked())
            .await
            .unwrap();
    }
    let reads_while_locked = clock.reads.load(Ordering::SeqCst);
    let renewal = {
        let store = store.clone();
        let resource = resource.clone();
        let fence = fence.clone();
        tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(store.renew_fence(&resource, &fence, 100))
        })
    };
    let sampled_before_lock = {
        let clock = clock.clone();
        tokio::task::spawn_blocking(move || {
            clock.wait_for_read_after(reads_while_locked, Duration::from_millis(100))
        })
        .await
        .unwrap()
    };
    clock.advance(10);
    clock.release();
    validation.await.unwrap().unwrap();
    let renewal = renewal.await.unwrap();
    assert!(!sampled_before_lock, "renewal sampled time before locking");
    assert_eq!(renewal.unwrap_err(), StoreError::FenceRejected);
}
