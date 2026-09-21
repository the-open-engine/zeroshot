//! Failed process cleanup retains both the workspace and its active identity lease.

use std::process::Stdio;

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use crate::execution::process::HostedProcessIdentity;
use crate::execution::process::test_support::KillCapabilityGuard;
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_runner::{NodeHandle, NodeRunRequest, NodeRunner, NodeRunnerError};

struct IdleRunner;

#[async_trait]
impl NodeRunner for IdleRunner {
    async fn start(&self, _: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        Err(NodeRunnerError::Driver)
    }

    async fn close_run(&self, _: &RunId) {}
}

fn writer(pool: HostedProcessPool) -> HostedProcessIdentity {
    pool.identity(HostedProcessScope::Writer).assert_value()
}

struct CleanupFixture {
    directory: TestDirectory,
    seed: HostedProcessPool,
    pools: ActiveRunProcessPools,
    state: Arc<ProductionCapsuleState>,
    active: Mutex<BTreeMap<RunId, Arc<ProductionCapsuleState>>>,
    run_id: RunId,
    identity: HostedProcessIdentity,
}

impl CleanupFixture {
    fn new() -> Self {
        let directory = TestDirectory::new("delivery-identity-lease-cleanup");
        let run_root = directory.child("run");
        std::fs::create_dir(&run_root).assert_value();
        std::fs::write(run_root.join("candidate"), "preserved work\n").assert_value();
        let seed = HostedProcessPool::new(128_002, 128_002, 129_000, 129_000).assert_value();
        let pools = ActiveRunProcessPools::new(seed).assert_value();
        let lease = pools.acquire().assert_value();
        let identity = writer(lease.process_pool());
        identity.prepare_command_domain().assert_value();
        let (loss, _) = watch::channel(false);
        let run_id = RunId::new("delivery-lease-cleanup");
        let state = Arc::new(ProductionCapsuleState {
            endpoint: OnceLock::from(Arc::new(NativeCapsuleNodeEndpoint::new(Arc::new(
                IdleRunner,
            )))),
            run_root_identity: OnceLock::from(WorkspaceIdentity::capture(&run_root).assert_value()),
            run_root,
            checkpoint_directory: directory.child("checkpoints"),
            recovery_path: directory.child("recovery.json"),
            delivery_run_id: run_id.clone(),
            inherited_retained_workspace: false,
            process_pool: Mutex::new(Some(lease)),
            portable_processes: false,
            _loss_sender: loss,
            cleanup_turn: Mutex::new(false),
        });
        let active = Mutex::new(BTreeMap::from([(run_id.clone(), state.clone())]));
        Self {
            directory,
            seed,
            pools,
            state,
            active,
            run_id,
            identity,
        }
    }

    fn helper(&self) -> tokio::process::Child {
        let mut command = tokio::process::Command::new("/usr/bin/sleep");
        command
            .arg("60")
            .env_clear()
            .env("GH_TOKEN", "test-only-delivery-lease-credential")
            .current_dir("/")
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        self.identity.configure_command(&mut command);
        command.spawn().assert_value()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn root_failed_delivery_cleanup_retains_lease_until_processes_are_gone() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only delivery identity lease test skipped");
        return;
    }
    let fixture = CleanupFixture::new();
    let mut helper = fixture.helper();
    let pid = helper.id().assert_value();
    let failure = {
        let _denied = KillCapabilityGuard::suspend();
        cleanup_state(
            &fixture.run_id,
            &fixture.state,
            &fixture.active,
            RunRuntimeExit::Completed,
        )
        .await
    };
    assert!(failure.is_err());
    assert_eq!(
        std::fs::read_to_string(fixture.state.run_root.join("candidate")).assert_value(),
        "preserved work\n"
    );
    assert!(fixture.active.lock().await.contains_key(&fixture.run_id));
    assert!(fixture.state.process_pool.lock().await.is_some());
    let another = fixture.pools.acquire().assert_value();
    assert_ne!(writer(another.process_pool()).uid(), fixture.identity.uid());

    // A restarted registry can select the numeric slot, but allocation must refuse its live UID
    // before passing a checkout credential to any process under that identity.
    let restarted = ActiveRunProcessPools::new(fixture.seed).assert_value();
    let reused = restarted.acquire().assert_value();
    let reused_identity = writer(reused.process_pool());
    assert_eq!(reused_identity.uid(), fixture.identity.uid());
    assert!(reused_identity.prepare_command_domain().is_err());
    assert!(std::path::Path::new(&format!("/proc/{pid}")).exists());
    drop(reused);

    cleanup_state(
        &fixture.run_id,
        &fixture.state,
        &fixture.active,
        RunRuntimeExit::Completed,
    )
    .await
    .assert_value();
    let _ = tokio::time::timeout(Duration::from_secs(2), helper.wait())
        .await
        .assert_value();
    assert!(!fixture.state.run_root.exists());
    assert!(!fixture.active.lock().await.contains_key(&fixture.run_id));
    assert!(fixture.state.process_pool.lock().await.is_none());
    let next = fixture.pools.acquire().assert_value();
    let next_identity = writer(next.process_pool());
    assert_eq!(next_identity.uid(), fixture.identity.uid());
    next_identity.prepare_command_domain().assert_value();
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    assert!(fixture.directory.path().exists());
}
