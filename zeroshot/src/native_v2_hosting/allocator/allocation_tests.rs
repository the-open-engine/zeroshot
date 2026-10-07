//! Allocation failure and cancellation retain ownership until checkout helpers are gone.

use std::os::unix::fs::PermissionsExt as _;
use std::sync::atomic::{AtomicU32, Ordering};

use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;
use crate::execution::process::test_support::KillCapabilityGuard;
use crate::native_v2_candidate::test_support::{TestDirectory, admit, full_graph, success_node};

static NEXT_IDENTITY: AtomicU32 = AtomicU32::new(3_000_000);

struct AllocationFixture {
    root: TestDirectory,
    allocator: ProductionCapsuleAllocator,
    admitted: AdmittedRun,
    run_id: RunId,
    identity: HostedProcessIdentity,
}

impl AllocationFixture {
    async fn new(wait_for_cancellation: bool) -> Self {
        let root = TestDirectory::new("allocation-helper-ownership");
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755))
            .assert_value();
        std::fs::create_dir(root.child("runs")).assert_value();
        let action = if wait_for_cancellation {
            "while :; do /usr/bin/sleep 1; done"
        } else {
            "printf 'checkout deliberately failed after spawning a helper\\n' >&2; exit 42"
        };
        let program = root.write_executable(
            "git-wrapper",
            &format!(
                r#"#!/bin/sh
set -eu
case " $* " in
  *" fetch "*)
    if [ ! -s "$0.pid" ]; then
      /usr/bin/setsid /bin/sh -c 'echo "$$" > "$1"; exec /usr/bin/sleep 60' helper "$0.pid" </dev/null >/dev/null 2>&1 &
      while [ ! -s "$0.pid" ]; do /usr/bin/sleep 0.01; done
    fi
    {action}
    ;;
esac
exec /usr/bin/git "$@"
"#
            ),
        );
        let marker = root.write("git-wrapper.pid", "");
        std::fs::set_permissions(marker, std::fs::Permissions::from_mode(0o666)).assert_value();
        let uid = NEXT_IDENTITY.fetch_add(1_000_000, Ordering::Relaxed);
        let unused_harness = PathBuf::from("/usr/bin/false");
        let allocator = ProductionCapsuleAllocator::new(ProductionCapsuleConfig {
            workspace_storage: None,
            storage_root: root.path().to_owned(),
            copilot_executable: unused_harness.clone(),
            codex_executable: unused_harness.clone(),
            claude_executable: unused_harness.to_string_lossy().into_owned(),
            claude_prefix_arguments: Vec::new(),
            claude_process_environment: ClaudeProcessEnvironment::default(),
            executable_search_path: "/usr/local/bin:/usr/bin:/bin".to_owned(),
            git_program: program,
            gh_program: PathBuf::from("/usr/bin/false"),
            process_pool: HostedProcessPool::new(uid - 1, uid - 1, uid).assert_value(),
            operator_diagnostics: Arc::new(OperatorDiagnosticStore::default()),
        })
        .assert_value()
        .with_test_filesystem_and_source(root.child("unused-remote.git"), production_filesystem);
        let lease = allocator.process_pools.acquire().assert_value();
        let identity = lease
            .process_pool()
            .identity(HostedProcessScope::Writer)
            .assert_value();
        drop(lease);
        let admitted = admit(
            serde_json::from_value(json!({
                "title":"Allocation ownership regression",
                "graph":full_graph(vec![
                    json!({
                        "kind":"step","name":"work","worker":"agent.work@1",
                        "instructions":"This node must never start.",
                        "input":{"kind":"null"},"output":{"kind":"null"},
                        "inputBindings":[],"writeBindings":[],"attempts":1
                    }),
                    success_node()
                ]),
                "initialInput":null,
                "runtime":{
                    "harness":"codex","provider":"openai","size":"small",
                    "nodes":{"work":{
                        "kind":"agent","model":"unused-model","sessionScope":"execution",
                        "connections":{}
                    }}
                },
                "source":{
                    "repository":"acme/project","branch":"main",
                    "revision":"1111111111111111111111111111111111111111"
                },
                "submissionKey":"allocation-helper-ownership"
            }))
            .assert_value(),
        )
        .await;
        Self {
            root,
            allocator,
            admitted,
            run_id: RunId::new("allocation-helper-ownership"),
            identity,
        }
    }

    fn request(&self) -> CapsuleAllocationRequest<'_> {
        crate::native_v2_candidate::test_support::allocation_request(
            &self.run_id,
            &self.admitted,
            Some("checkout-test-only-credential"),
        )
    }

    fn helper_pid(&self) -> Option<libc::pid_t> {
        std::fs::read_to_string(self.root.child("git-wrapper.pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    async fn wait_for_helper(&self) -> libc::pid_t {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(pid) = self.helper_pid() {
                    return pid;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .assert_value()
    }

    async fn destroy(&self) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        self.allocator
            .destroy_or_confirm_absent(&self.run_id, RunRuntimeExit::RuntimeLost)
            .await
    }

    async fn retained_state(&self) -> Arc<ProductionCapsuleState> {
        let state = self.allocator.active.lock().await[&self.run_id].clone();
        assert!(state.endpoint.get().is_none());
        assert!(state.process_pool.lock().await.is_some());
        state
    }

    async fn assert_released(&self, pid: libc::pid_t) {
        assert!(!helper_running(pid));
        assert!(!self.allocator.run_path(&self.run_id).exists());
        assert!(
            !self
                .allocator
                .active
                .lock()
                .await
                .contains_key(&self.run_id)
        );
        let lease = self.allocator.process_pools.acquire().assert_value();
        let reused = lease
            .process_pool()
            .identity(HostedProcessScope::Writer)
            .assert_value();
        assert_eq!(reused.uid(), self.identity.uid());
        reused.prepare_command_domain().assert_value();
    }
}

impl Drop for AllocationFixture {
    fn drop(&mut self) {
        if let Some(pid) = self.helper_pid().filter(|pid| helper_running(*pid)) {
            // SAFETY: this PID comes only from the bounded test helper's private marker.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

fn helper_running(pid: libc::pid_t) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|status| status.split_whitespace().nth(2) != Some("Z"))
}

#[tokio::test(flavor = "current_thread")]
async fn root_failed_checkout_retains_helper_workspace_and_lease_until_cleanup_succeeds() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only allocation cleanup test skipped");
        return;
    }
    let fixture = AllocationFixture::new(false).await;
    let failure = {
        let _denied = KillCapabilityGuard::suspend();
        fixture.allocator.allocate(fixture.request()).await
    };
    assert!(matches!(
        failure,
        Err(CapsuleAllocationUnavailable::SourceCheckout)
    ));
    let pid = fixture.helper_pid().assert_value();
    assert!(helper_running(pid));
    // SAFETY: getsid only observes the helper identified by the private marker.
    assert_eq!(unsafe { libc::getsid(pid) }, pid);
    let state = fixture.retained_state().await;
    assert!(state.run_root.join("workspace/.git").is_dir());
    assert!(state.process_pool.lock().await.is_some());
    let other = fixture.allocator.process_pools.acquire().assert_value();
    assert_ne!(
        other
            .process_pool()
            .identity(HostedProcessScope::Writer)
            .assert_value()
            .uid(),
        fixture.identity.uid()
    );
    {
        let _denied = KillCapabilityGuard::suspend();
        assert!(fixture.destroy().await.is_err());
    }
    assert!(helper_running(pid));
    assert!(state.run_root.exists());
    assert!(state.process_pool.lock().await.is_some());
    fixture.destroy().await.assert_value();
    assert!(state.process_pool.lock().await.is_none());
    fixture.assert_released(pid).await;
}

#[tokio::test(flavor = "current_thread")]
async fn root_cancelled_checkout_remains_destroyable_before_its_uid_is_reused() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only cancelled allocation cleanup test skipped");
        return;
    }
    let fixture = AllocationFixture::new(true).await;
    let mut allocation = Box::pin(fixture.allocator.allocate(fixture.request()));
    let pid = tokio::select! {
        _ = &mut allocation => panic!("checkout returned before cancellation"),
        pid = fixture.wait_for_helper() => pid,
    };
    // A different accepted run can be stopped before it allocates, without waiting for this one.
    let unrelated = RunId::new("cancelled-before-allocation");
    let cleanup = tokio::time::timeout(
        Duration::from_secs(1),
        fixture
            .allocator
            .destroy_or_confirm_absent(&unrelated, RunRuntimeExit::ForceStopped),
    )
    .await;
    drop(allocation);
    assert!(helper_running(pid));
    let state = fixture.retained_state().await;
    assert!(state.run_root.exists());
    assert!(state.process_pool.lock().await.is_some());
    fixture.destroy().await.assert_value();
    fixture.assert_released(pid).await;
    cleanup.assert_value().assert_value();
}

#[tokio::test(flavor = "current_thread")]
async fn preexisting_run_directory_is_preserved_after_allocation_and_destroy_fail() {
    let fixture = AllocationFixture::new(false).await;
    let run_root = fixture.allocator.run_path(&fixture.run_id);
    std::fs::create_dir(&run_root).assert_value();
    let sentinel = run_root.join("retained-work");
    std::fs::write(&sentinel, "must survive allocation refusal").assert_value();
    assert!(matches!(
        fixture.allocator.allocate(fixture.request()).await,
        Err(CapsuleAllocationUnavailable::Runtime)
    ));
    assert!(fixture.destroy().await.is_err());
    assert_eq!(
        std::fs::read_to_string(sentinel).assert_value(),
        "must survive allocation refusal"
    );
    let state = fixture.retained_state().await;
    assert!(state.process_pool.lock().await.is_some());
    assert!(fixture.helper_pid().is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn root_idle_domain_refusal_preserves_unowned_helpers_and_existing_work() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only allocation idle-domain refusal test skipped");
        return;
    }
    for preexisting_workspace in [false, true] {
        let fixture = AllocationFixture::new(false).await;
        fixture.identity.prepare_command_domain().assert_value();
        let mut command = tokio::process::Command::new("/usr/bin/sleep");
        fixture.identity.configure_command(&mut command);
        let mut previous_helper = command
            .env_clear()
            .current_dir("/")
            .arg("60")
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .assert_value();
        let pid = previous_helper
            .id()
            .assert_value()
            .try_into()
            .assert_value();
        let run_root = fixture.allocator.run_path(&fixture.run_id);
        let sentinel = run_root.join("previous-work");
        if preexisting_workspace {
            std::fs::create_dir(&run_root).assert_value();
            std::fs::write(&sentinel, "belongs to an earlier allocation").assert_value();
        }
        assert!(matches!(
            fixture.allocator.allocate(fixture.request()).await,
            Err(CapsuleAllocationUnavailable::Runtime)
        ));
        assert!(
            !fixture
                .allocator
                .active
                .lock()
                .await
                .contains_key(&fixture.run_id)
        );
        assert!(
            fixture.helper_pid().is_none(),
            "checkout must not have started"
        );
        let destruction = fixture.destroy().await;
        if preexisting_workspace {
            assert!(destruction.is_err());
            assert_eq!(
                std::fs::read_to_string(&sentinel).assert_value(),
                "belongs to an earlier allocation"
            );
        } else {
            destruction.assert_value();
            assert!(!run_root.exists());
        }
        assert!(helper_running(pid), "unowned helper must remain untouched");
        previous_helper.kill().await.assert_value();
        fixture.identity.prepare_command_domain().assert_value();
    }
}
