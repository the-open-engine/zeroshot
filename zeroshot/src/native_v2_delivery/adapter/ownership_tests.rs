//! The delivery identity is not returned to a writer until its process domain is empty.

use std::process::Stdio;
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;

use openengine_cluster_protocol::{DeclaredConnections, FieldName, RunId, WorkerRef};
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use crate::execution::process::{HostedProcessIdentity, HostedProcessPool, HostedProcessScope};
use crate::execution::process::test_support::KillCapabilityGuard;
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_runner::{NativeNodeRunner, NodeRunner, test_support};

#[derive(Clone, Copy)]
enum Fault {
    Panic,
    CleanupDenied,
}

struct FaultSession {
    inner: DeliverySession,
    identity: HostedProcessIdentity,
    fault: Fault,
    pid: AtomicU32,
    capability: Mutex<Option<KillCapabilityGuard>>,
}

impl FaultSession {
    fn start_helper(&self) {
        let mut command = tokio::process::Command::new("/usr/bin/sleep");
        command
            .arg("60")
            .env_clear()
            .env("GH_TOKEN", "test-only-delivery-credential")
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        self.identity.configure_command(&mut command);
        let child = command.spawn().assert_value();
        self.pid.store(child.id().assert_value(), Ordering::SeqCst);
        // The adapter owns identity cleanup. Retain no child guard that could hide a missed fence.
        drop(child);
    }

    fn helper_is_live(&self) -> bool {
        let pid = self.pid.load(Ordering::SeqCst);
        pid != 0
            && std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                stat.rsplit_once(") ")
                    .is_some_and(|(_, rest)| !matches!(rest.as_bytes().first(), Some(b'Z' | b'X')))
            })
    }

    fn restore_capability(&self) {
        drop(self.capability.lock().assert_value().take());
    }
}

#[async_trait]
impl NodeSession for FaultSession {
    fn as_any(&self) -> &dyn Any {
        self.start_helper();
        match self.fault {
            Fault::Panic => panic!("injected delivery panic after helper startup"),
            Fault::CleanupDenied => {
                *self.capability.lock().assert_value() = Some(KillCapabilityGuard::suspend());
                &self.inner
            }
        }
    }

    async fn is_live(&self) -> bool {
        true
    }

    async fn close(&self) {}
}

struct FenceProbe {
    adapter: NativeV2DeliveryAdapter,
    session: Arc<FaultSession>,
    helper_survived_fence: AtomicBool,
}

#[async_trait]
impl SessionFactory for FenceProbe {
    async fn open(
        &self,
        _: &NodeInvocation,
        _: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        Ok(self.session.clone())
    }
}

#[async_trait]
impl NodeDriver for FenceProbe {
    async fn run(
        &self,
        mut invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let mode = DeliveryMode::PullRequest;
        invocation.role = NodeRole::GitDelivery;
        invocation.node.worker = WorkerRef::new(GIT_DELIVERY_PR_WORKER_REF).assert_value();
        invocation.node.binding = NodeRuntimeBinding::GitDelivery {
            connections: DeclaredConnections::empty(),
        };
        // A malformed input provides an ordinary outcome that failed cleanup must override.
        invocation.node.input = serde_json::json!(17);
        invocation.response = NodeResponseContract::Verifier {
            output: contract::delivery_result_schema(mode).assert_value(),
            signals: BTreeMap::from([(
                FieldName::new(DELIVERY_SIGNAL_FIELD).assert_value(),
                contract::delivery_signal_labels(mode).assert_value(),
            )]),
            diagnostic: contract::delivery_diagnostic_schema().assert_value(),
        };
        let result = self.adapter.run(invocation, control).await;
        self.session.restore_capability();
        self.helper_survived_fence
            .store(self.session.helper_is_live(), Ordering::SeqCst);
        // Fault-injection cleanup remains test-owned after recording the adapter's actual result.
        assert!(self.session.identity.cleanup().await.proves_tree_empty());
        result
    }
}

fn probe(directory: &TestDirectory, fault: Fault) -> Arc<FenceProbe> {
    let uid = match fault {
        Fault::Panic => 121_002,
        Fault::CleanupDenied => 123_002,
    };
    let identity = HostedProcessPool::new(uid, uid, uid + 100, uid + 100)
        .assert_value()
        .identity(HostedProcessScope::Writer)
        .assert_value();
    let adapter = NativeV2DeliveryAdapter::new(
        NativeV2DeliveryConfig::for_hosted_workspace(
            DeliveryLineage::new(RunId::new("delivery-identity-fence"), false),
            directory.path().to_owned(),
            DeliveryTarget::new("acme/project", "main", "a".repeat(40)).assert_value(),
            identity,
        ),
        Arc::new(GhCliDeliveryAuthority::new(GhCliAuthorityConfig::hosted(
            directory.path().to_owned(),
        ))),
    )
    .with_trusted_github_token(Some(Arc::from("test-only-delivery-credential")));
    Arc::new(FenceProbe {
        adapter,
        session: Arc::new(FaultSession {
            inner: DeliverySession {
                workspace: directory.path().to_owned(),
                live: AtomicBool::new(true),
            },
            identity,
            fault,
            pid: AtomicU32::new(0),
            capability: Mutex::new(None),
        }),
        helper_survived_fence: AtomicBool::new(false),
    })
}

async fn run_probe(
    probe: Arc<FenceProbe>,
) -> Result<crate::native_v2_contract::NodeCompletion, NodeRunnerError> {
    let runner =
        NativeNodeRunner::new(&test_support::admitted(), probe.clone(), probe).assert_value();
    let mut handle = runner
        .start(test_support::request("delivery-fence", "worker", (1, 1)))
        .await
        .assert_value();
    let _output = handle.take_initial_output().assert_value();
    let result = handle.completion().await;
    tokio::time::timeout(
        Duration::from_secs(2),
        runner.close_run(&openengine_cluster_protocol::RunId::new("delivery-fence")),
    )
    .await
    .assert_value_with("delivery failure must settle runner activity before run close");
    result
}

#[tokio::test(flavor = "current_thread")]
async fn root_delivery_panic_reaps_helpers_and_settles_runner_activity() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only delivery panic cleanup test skipped");
        return;
    }
    let directory = TestDirectory::new("delivery-panic-cleanup");
    let probe = probe(&directory, Fault::Panic);
    let Err(NodeRunnerError::DriverDetail(message)) = run_probe(probe.clone()).await else {
        panic!("delivery panic must settle as a sanitized driver failure");
    };
    assert_eq!(message, "Git delivery panicked");
    assert!(!message.contains("injected"));
    assert!(!message.contains("test-only-delivery-credential"));
    assert!(probe.session.pid.load(Ordering::SeqCst) > 0);
    assert!(!probe.helper_survived_fence.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "current_thread")]
async fn root_delivery_cleanup_refusal_overrides_an_ordinary_outcome() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only delivery cleanup refusal test skipped");
        return;
    }
    let directory = TestDirectory::new("delivery-cleanup-refusal");
    let probe = probe(&directory, Fault::CleanupDenied);
    assert!(matches!(
        run_probe(probe.clone()).await,
        Err(NodeRunnerError::CleanupUnconfirmed)
    ));
    assert!(probe.helper_survived_fence.load(Ordering::SeqCst));
    assert!(!probe.session.helper_is_live());
}
