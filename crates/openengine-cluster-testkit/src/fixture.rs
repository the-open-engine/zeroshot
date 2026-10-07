//! Shared in-process fixture wiring for integration tests that need the transport-neutral
//! `Dispatcher` (and its backend) directly — for example to call `Dispatcher::watch` or
//! `ClusterBackend::watch` with an explicit queue capacity — rather than only a wrapped
//! `ClusterClient`.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use openengine_cluster_client::{ClusterClient, InProcessTransport};
use openengine_cluster_protocol::{
    BackendFault, BoundedString256, FaultAction, FaultCode, FaultConsequence,
    FaultRetryDisposition, FaultSeverity, FaultSourceFrame,
};
use openengine_cluster_server::admission::AdmissionCoordinator;
use openengine_cluster_server::{ConnectionContext, Dispatcher};
pub(crate) use crate::assertions::{AssertAt, AssertValue};
use serde_json::Value;

use crate::admission::{InMemoryAdmissionStore, ScriptedOutcome, ScriptedVerifier};

pub(crate) trait JsonAt {
    fn assert_key(&self, key: &str) -> &Value;
    fn assert_key_mut(&mut self, key: &str) -> &mut Value;
}

impl JsonAt for Value {
    fn assert_key(&self, key: &str) -> &Value {
        self.get(key)
            .assert_value_with("expected JSON object field")
    }

    fn assert_key_mut(&mut self, key: &str) -> &mut Value {
        self.get_mut(key)
            .assert_value_with("expected mutable JSON object field")
    }
}

/// Process-unique temporary directory removed on drop.
pub struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    pub fn new(prefix: &str) -> io::Result<Self> {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{suffix}", std::process::id()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }

    #[must_use]
    pub fn for_test(prefix: &str) -> Self {
        let result = Self::new(prefix);
        assert!(result.is_ok(), "temporary test directory must be created");
        let mut directories = result.into_iter().collect::<Vec<_>>();
        directories.swap_remove(0)
    }

    #[must_use]
    pub fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Writes a Unix executable and waits out writable descriptors inherited by concurrent forks.
///
/// The lock handoff closes the `fork`/`CLOEXEC` window that can otherwise make an immediate spawn
/// fail with `ETXTBSY`.
#[cfg(unix)]
pub fn write_executable(path: &Path, contents: impl AsRef<[u8]>, mode: u32) -> io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(contents.as_ref())?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    finish_executable_write(file, path)
}

#[cfg(unix)]
fn finish_executable_write(file: std::fs::File, path: &Path) -> io::Result<()> {
    fs2::FileExt::lock_exclusive(&file)?;
    drop(file);

    let file = std::fs::File::open(path)?;
    fs2::FileExt::lock_shared(&file)
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn duplicated_writer_holds_the_lock_handoff_until_it_closes() {
        let directory = TemporaryDirectory::for_test("executable-lock-handoff");
        let path = directory.path("fixture");
        let writer = std::fs::File::create(&path).expect("create executable fixture");
        let inherited_writer = writer.try_clone().expect("duplicate executable writer");
        let waiting_path = path.clone();
        let (finished, result) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            finished
                .send(finish_executable_write(writer, &waiting_path))
                .expect("report lock handoff result");
        });

        let probe = std::fs::File::open(&path).expect("open lock probe");
        loop {
            match fs2::FileExt::try_lock_shared(&probe) {
                Ok(()) => {
                    fs2::FileExt::unlock(&probe).expect("unlock probe");
                    match result.try_recv() {
                        Ok(finished) => {
                            panic!("lock handoff finished before taking its lock: {finished:?}")
                        }
                        Err(mpsc::TryRecvError::Empty) => std::thread::yield_now(),
                        Err(mpsc::TryRecvError::Disconnected) => {
                            panic!("lock handoff result sender disconnected")
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("probe lock failed: {error}"),
            }
        }
        assert!(matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty)));

        drop(inherited_writer);
        result
            .recv()
            .expect("receive lock handoff result")
            .expect("lock handoff should succeed");
        waiter.join().expect("join lock handoff waiter");
    }
}

pub type FixtureBackend = AdmissionCoordinator<ScriptedVerifier, InMemoryAdmissionStore>;
pub type FixtureClient = ClusterClient<InProcessTransport<FixtureBackend>>;

/// Builds a fresh `ClusterClient`/`Dispatcher`/backend/verifier/store fixture wired to
/// `outcomes`. The returned client, dispatcher, and backend are cheap `Arc`-backed handles onto
/// the same underlying coordinator and store. The sole construction site for this fixture shape;
/// callers that only need a subset (for example just the client and store) destructure and
/// discard the rest rather than re-deriving it.
#[must_use]
pub fn dispatcher_fixture(
    outcomes: Vec<ScriptedOutcome>,
) -> (
    FixtureClient,
    Dispatcher<FixtureBackend>,
    FixtureBackend,
    Arc<ScriptedVerifier>,
    Arc<InMemoryAdmissionStore>,
) {
    let verifier = Arc::new(ScriptedVerifier::new(outcomes));
    let store = Arc::new(InMemoryAdmissionStore::default());
    let backend = AdmissionCoordinator::from_shared(Arc::clone(&verifier), Arc::clone(&store));
    let dispatcher = Dispatcher::new(backend.clone(), ConnectionContext::default());
    let client = ClusterClient::new(InProcessTransport::new(dispatcher.clone()));
    (client, dispatcher, backend, verifier, store)
}

/// A valid, deterministic `BackendFault` for reuse by goldens and the testkit `backend_faults`
/// conformance test. `event_id` is the sole varying input so callers can produce distinct fault
/// events.
#[must_use]
pub fn sample_backend_fault(event_id: &str) -> BackendFault {
    BackendFault {
        event_id: BoundedString256::new(event_id)
            .assert_value_with("fixture event id must be valid"),
        execution_ref: None,
        code: FaultCode::Unavailable,
        consequence: FaultConsequence::TurnFailed,
        retry: FaultRetryDisposition::RetryableAfterBackoff,
        action: FaultAction::Retry,
        severity: FaultSeverity::Error,
        summary: BoundedString256::new("upstream worker unavailable")
            .assert_value_with("fixture summary must be valid"),
        source: vec![FaultSourceFrame {
            component: BoundedString256::new("worker-dispatch")
                .assert_value_with("fixture component must be valid"),
        }],
    }
}
