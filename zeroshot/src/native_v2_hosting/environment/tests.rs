use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::sync::atomic::AtomicU32;
use async_trait::async_trait;
use openengine_cluster_testkit::assertions::AssertValue;
use crate::execution::process::{HostedProcessPool, HostedProcessScope};
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_cloud::PreparationProgress;

static NEXT_UID: AtomicU32 = AtomicU32::new(50_000_000);

#[derive(Default)]
struct CapturedProgress(Mutex<Vec<String>>);
#[async_trait]
impl PreparationProgress for CapturedProgress {
    async fn log(&self, line: &str) -> Result<(), CapsuleAllocationUnavailable> {
        self.0.lock().assert_value().push(line.to_owned());
        Ok(())
    }
}

struct Fixture {
    root: TestDirectory,
    processes: EnvironmentProcesses,
    identity: HostedProcessIdentity,
    pool: HostedProcessPool,
    preparation: CapsulePreparation,
    log: Arc<CapturedProgress>,
    values: BTreeMap<String, String>,
    setup_home: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = TestDirectory::new("runtime-hooks");
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o755))
            .assert_value();
        let uid = NEXT_UID.fetch_add(100_000, Ordering::Relaxed);
        let pool = HostedProcessPool::new(uid, uid, uid + 10).assert_value();
        let identity = pool
            .identity(HostedProcessScope::Environment)
            .assert_value();
        let runtime = root.child("runtime");
        std::fs::create_dir(&runtime).assert_value();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o711)).assert_value();
        let setup_home =
            prepare_environment(root.path(), &runtime, identity, HookPhase::Setup).assert_value();
        let home =
            prepare_environment(root.path(), &runtime, identity, HookPhase::Startup).assert_value();
        let workspace = root.child("workspace");
        std::fs::create_dir(&workspace).assert_value();
        std::os::unix::fs::chown(&workspace, Some(uid), Some(uid)).assert_value();
        let runtime = serde_json::from_value(
            serde_json::json!({"harness":"codex","provider":"openai","size":"medium","nodes":{}}),
        )
        .assert_value();
        let log = Arc::new(CapturedProgress::default());
        let mut preparation = CapsulePreparation::quiet(&runtime).assert_value();
        preparation.progress = log.clone();
        let values = BTreeMap::from([
            ("PATH".to_owned(), search_path(root.path(), "/usr/bin:/bin")),
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            (
                "ZEROSHOT_TOOLS".to_owned(),
                tools_directory(root.path()).to_string_lossy().into_owned(),
            ),
            (
                "PRIVATE_TOKEN".to_owned(),
                "test-secret-credential".to_owned(),
            ),
        ]);
        Self {
            root,
            processes: EnvironmentProcesses::default(),
            identity,
            pool,
            preparation,
            log,
            values,
            setup_home,
        }
    }
    async fn run(
        &self,
        phase: HookPhase,
        script: &str,
    ) -> Result<(), CapsuleAllocationUnavailable> {
        let mut values = self.values.clone();
        if matches!(phase, HookPhase::Setup) {
            values.insert(
                "HOME".to_owned(),
                self.setup_home.to_string_lossy().into_owned(),
            );
        }
        self.processes
            .run(HookRequest {
                phase,
                script: Some(script),
                directory: &self.root.child("workspace"),
                identity: self.identity,
                environment: &values,
                preparation: &self.preparation,
                redactions: &["test-secret-credential".to_owned()],
            })
            .await
    }
    async fn cleanup(&self) {
        self.processes.cleanup().await.assert_value();
        assert!(self.identity.cleanup().await.proves_tree_empty());
        self.processes.finish_output().await.assert_value();
    }
}

#[tokio::test]
async fn root_hooks_install_tools_start_unprivileged_and_keep_services_between_nodes() {
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let fixture = Fixture::new();
    fixture
        .run(
            HookPhase::Setup,
            concat!(
                "id -u > setup-uid; ",
                "printf '#!/bin/sh\necho installed\n' > \"$ZEROSHOT_TOOLS/bin/example\"; ",
                "chmod +x \"$ZEROSHOT_TOOLS/bin/example\"",
            ),
        )
        .await
        .assert_value();
    assert_eq!(
        std::fs::read_to_string(fixture.root.child("workspace/setup-uid"))
            .assert_value()
            .trim(),
        "0"
    );
    fixture
        .run(
            HookPhase::Startup,
            concat!(
                "example > installed; id -u > startup-uid; ",
                "setsid sh -c 'echo $$ > service-pid; exec sleep 60' & ",
                "while [ ! -s service-pid ]; do sleep 0.01; done",
            ),
        )
        .await
        .assert_value();
    assert_eq!(
        std::fs::metadata(fixture.root.child("workspace/installed"))
            .assert_value()
            .uid(),
        fixture.identity.uid()
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.child("workspace/installed")).assert_value(),
        "installed\n"
    );
    let pid: i32 = std::fs::read_to_string(fixture.root.child("workspace/service-pid"))
        .assert_value()
        .trim()
        .parse()
        .assert_value();
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
    assert!(
        fixture
            .pool
            .identity(HostedProcessScope::WriterExecution(1))
            .assert_value()
            .cleanup()
            .await
            .proves_tree_empty()
    );
    assert!(
        fixture
            .pool
            .identity(HostedProcessScope::Delivery)
            .assert_value()
            .cleanup()
            .await
            .proves_tree_empty()
    );
    assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
    fixture.cleanup().await;
    assert_ne!(unsafe { libc::kill(pid, 0) }, 0);
}

#[tokio::test]
async fn root_hook_failure_is_redacted_and_allocation_cancellation_keeps_cleanup_authority() {
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let fixture = Fixture::new();
    assert_eq!(
        fixture
            .run(HookPhase::Setup, "echo \"$PRIVATE_TOKEN\" >&2; exit 23")
            .await,
        Err(CapsuleAllocationUnavailable::EnvironmentSetup)
    );
    let logs = fixture.log.0.lock().assert_value().join("\n");
    assert!(logs.contains("[REDACTED]"));
    assert!(!logs.contains("test-secret-credential"));
    let mut run = Box::pin(fixture.run(HookPhase::Setup, "echo $$ > running-pid; exec sleep 60"));

    tokio::select! {
        result = &mut run => panic!("unexpected hook result {result:?}"),
        () = tokio::time::sleep(Duration::from_millis(100)) => {}
    }
    // Dropping a cancelled allocation future leaves the registered root child available to cleanup.
    drop(run);
    fixture.cleanup().await;
}

#[tokio::test]
async fn output_redacts_secrets_split_across_reads_and_bounds_unterminated_lines() {
    use tokio::io::AsyncWriteExt;
    let log = Arc::new(CapturedProgress::default());
    let output = output::HookOutput::new(log.clone(), vec!["a-secret-value".to_owned()], "setup");
    let (mut writer, reader) = tokio::io::duplex(32);
    let drain = tokio::spawn(output::drain(reader, output));
    writer.write_all(b"before a-sec").await.assert_value();
    tokio::task::yield_now().await;
    writer
        .write_all(b"ret-value after\0\n")
        .await
        .assert_value();
    writer
        .write_all(&vec![b'x'; 20 * 1024])
        .await
        .assert_value();
    writer.shutdown().await.assert_value();
    drain.await.assert_value().assert_value();
    let lines = log.0.lock().assert_value();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0], "[setup] before [REDACTED] after�");
    assert_eq!(lines[1], "[setup] [oversized output line omitted]");
}

#[tokio::test]
async fn output_failure_is_reported_instead_of_silently_dispatching_agents() {
    use tokio::io::AsyncWriteExt;
    struct FailedProgress;
    #[async_trait]
    impl PreparationProgress for FailedProgress {
        async fn log(&self, _line: &str) -> Result<(), CapsuleAllocationUnavailable> {
            Err(CapsuleAllocationUnavailable::Runtime)
        }
    }
    let output = output::HookOutput::new(Arc::new(FailedProgress), vec![], "startup");
    let (mut writer, reader) = tokio::io::duplex(32);
    writer
        .write_all(b"important failure\n")
        .await
        .assert_value();
    writer.shutdown().await.assert_value();
    assert_eq!(
        output::drain(reader, output).await,
        Err(CapsuleAllocationUnavailable::Runtime)
    );
}

#[tokio::test]
async fn output_timeout_joins_cancelled_drainer_before_releasing_ownership() {
    struct DrainerOwner(Arc<AtomicBool>);
    impl Drop for DrainerOwner {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let finished = Arc::new(AtomicBool::new(false));
    let owner = DrainerOwner(finished.clone());
    let (ready, started) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let _owner = owner;
        ready.send(()).assert_value();
        std::future::pending::<Result<(), CapsuleAllocationUnavailable>>().await
    });
    let processes = EnvironmentProcesses::default();
    processes
        .output
        .lock()
        .assert_value()
        .push(Arc::new(AsyncMutex::new(Some(task))));
    started.await.assert_value();
    assert!(processes.finish_output().await.is_err());
    assert!(
        finished.load(Ordering::Acquire),
        "cleanup must join the aborted output owner"
    );
    processes.finish_output().await.assert_value();
}

#[tokio::test]
async fn root_setup_and_startup_have_separate_caches_and_share_installed_tools() {
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let fixture = Fixture::new();
    for phase in [HookPhase::Setup, HookPhase::Startup] {
        fixture
            .run(
                phase,
                concat!(
                    "mkdir -p \"$HOME/.cache\"; touch \"$HOME/.cache/cache-entry\"; ",
                    "printf '%s\n' \"$HOME\" \"$ZEROSHOT_TOOLS\" > \"$(id -u)-paths\"",
                ),
            )
            .await
            .assert_value();
    }
    let paths = [0, fixture.identity.uid()].map(|uid| {
        let text = std::fs::read_to_string(fixture.root.child(&format!("workspace/{uid}-paths")))
            .assert_value();
        let paths: Vec<PathBuf> = text.lines().map(PathBuf::from).collect();
        assert_eq!(paths.len(), 2);
        let metadata = std::fs::metadata(paths[0].join(".cache/cache-entry")).assert_value();
        assert_eq!(metadata.uid(), uid);
        paths
    });
    assert_ne!(paths[0][0], paths[1][0]);
    assert_eq!(paths[0][1], paths[1][1]);
    assert_eq!(paths[0][1], tools_directory(fixture.root.path()));
    assert_eq!(
        std::fs::metadata(&paths[0][0]).assert_value().mode() & 0o777,
        0o700
    );
    fixture.cleanup().await;
}
