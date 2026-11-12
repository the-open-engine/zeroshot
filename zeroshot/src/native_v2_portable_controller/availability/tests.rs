use super::*;
use crate::execution::platform::{private_file, FileAccess};
use std::io::Write;
use std::process::{Command, Stdio};

struct Fixture {
    _root: tempfile::TempDir,
    paths: PortableControllerPaths,
    id: RunId,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let storage = root.path().join("storage");
        platform::create_private_directory(&storage).unwrap();
        let paths = PortableControllerPaths::new(storage);
        let id = RunId::new("0199f33f-3b44-7d21-9000-000000000001");
        Self {
            _root: root,
            paths,
            id,
        }
    }

    fn ready(&self, pid: u32, lease: &ControllerLease) {
        let ready = PortableControllerReady {
            kind: "zeroshot.portable-controller-ready/v1".into(),
            run_id: self.id.clone(),
            socket: self.paths.socket(),
            pid,
            lease_identity: Some(lease.identity()),
        };
        let mut file = private_file(&self.paths.ready(), FileAccess::ReadWrite).unwrap();
        file.set_len(0).unwrap();
        file.write_all(&serde_json::to_vec(&ready).unwrap())
            .unwrap();
    }

    async fn state(&self) -> ControllerObservation {
        match check_controller(&self.paths, &self.id).await {
            ControllerAvailability::Unavailable(observed) => observed,
            _ => panic!("expected an unavailable controller"),
        }
    }
}

fn exited_pid() -> u32 {
    // Run only the test listing in an owned process, then reap it on every platform.
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--list")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    assert!(child.wait().unwrap().success());
    pid
}

#[tokio::test]
async fn absence_requires_the_original_released_lease_and_absent_process() {
    let fixture = Fixture::new();
    assert_eq!(fixture.state().await.state, ControllerState::Unknown);
    assert!(!fixture.paths.lease().exists());
    let lease = ControllerLease::acquire(fixture.paths.lease()).unwrap();
    fixture.ready(exited_pid(), &lease);
    assert_eq!(
        fixture.state().await.state,
        ControllerState::Unknown,
        "held lease prevents a loss claim"
    );
    drop(lease);
    let before = std::fs::read(fixture.paths.ready()).unwrap();
    let observed = fixture.state().await;
    assert_eq!(observed.state, ControllerState::Lost);
    assert!(observed.observed_at.get() > 1);
    assert_eq!(before, std::fs::read(fixture.paths.ready()).unwrap());
}

#[tokio::test]
async fn live_or_reused_pid_and_replaced_lease_are_unknown() {
    let fixture = Fixture::new();
    let lease = ControllerLease::acquire(fixture.paths.lease()).unwrap();
    fixture.ready(std::process::id(), &lease);
    drop(lease);
    assert_eq!(fixture.state().await.state, ControllerState::Unknown);
    let lease = ControllerLease::acquire(fixture.paths.lease()).unwrap();
    fixture.ready(exited_pid(), &lease);
    std::fs::rename(
        fixture.paths.lease(),
        fixture.paths.storage().join("old.lock"),
    )
    .unwrap();
    let replacement = ControllerLease::acquire(fixture.paths.lease()).unwrap();
    drop(replacement);
    assert_eq!(fixture.state().await.state, ControllerState::Unknown);
    assert!(!lease.is_intact());
}

#[tokio::test]
async fn legacy_or_mismatched_readiness_is_unknown() {
    let fixture = Fixture::new();
    let lease = ControllerLease::acquire(fixture.paths.lease()).unwrap();
    fixture.ready(exited_pid(), &lease);
    drop(lease);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.paths.ready()).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("leaseIdentity");
    std::fs::write(fixture.paths.ready(), serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(fixture.state().await.state, ControllerState::Unknown);
    value["runId"] = serde_json::json!("another-run");
    std::fs::write(fixture.paths.ready(), serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(fixture.state().await.state, ControllerState::Unknown);
}

#[tokio::test]
async fn stalled_endpoint_is_bounded_and_never_proves_loss() {
    let fixture = Fixture::new();
    let lease = ControllerLease::acquire(fixture.paths.lease()).unwrap();
    fixture.ready(std::process::id(), &lease);
    let listener = super::super::transport::Listener::bind(&fixture.paths.socket()).unwrap();
    let stalled = tokio::spawn(async move {
        let _stream = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let observed = tokio::time::timeout(PROBE_TIMEOUT + Duration::from_secs(2), fixture.state())
        .await
        .unwrap();
    assert_eq!(observed.state, ControllerState::Unknown);
    stalled.abort();
    let _ = stalled.await;
}

#[cfg(unix)]
#[tokio::test]
async fn permission_failures_are_unknown_and_read_only_leases_are_probeable() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let lease = ControllerLease::acquire(fixture.paths.lease()).unwrap();
    fixture.ready(exited_pid(), &lease);
    drop(lease);
    std::fs::set_permissions(
        fixture.paths.lease(),
        std::fs::Permissions::from_mode(0o400),
    )
    .unwrap();
    assert_eq!(fixture.state().await.state, ControllerState::Lost);
    std::fs::set_permissions(fixture.paths.lease(), std::fs::Permissions::from_mode(0o0)).unwrap();
    if unsafe { libc::geteuid() } != 0 {
        assert_eq!(fixture.state().await.state, ControllerState::Unknown);
    }
    std::fs::set_permissions(
        fixture.paths.lease(),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
}
