use std::collections::BTreeMap;
use std::fs;
use std::sync::Mutex;

use openengine_cluster_protocol::{RunConnectionValues, RunId};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};
use tokio::time::{sleep, timeout};

use super::*;
use crate::native_v2_admission::NativeV2Admission;
use crate::native_v2_candidate::test_support::allocation_request;
use crate::native_v2_contract::AdmittedRun;
use crate::native_v2_supervisor::RunEnvironment;

const WAIT_LIMIT: Duration = Duration::from_secs(10);
const FIXTURE_SECRET: &str = "local-hook-test-secret";

type LogAction = (&'static str, Box<dyn FnOnce() + Send>);

#[derive(Default)]
struct ObservedProgress {
    lines: Mutex<Vec<String>>,
    action: Mutex<Option<LogAction>>,
}

#[async_trait]
impl PreparationProgress for ObservedProgress {
    async fn log(&self, line: &str) -> Result<(), CapsuleAllocationUnavailable> {
        self.lines.lock().assert_value().push(line.to_owned());
        let mut action = self.action.lock().assert_value();
        if action.as_ref().is_some_and(|(trigger, _)| *trigger == line) {
            action.take().assert_value().1();
        }
        Ok(())
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    bootstrap: PortableControllerBootstrap,
    admitted: AdmittedRun,
    identity: WorkspaceIdentity,
    lease: Arc<ControllerLease>,
    progress: Arc<ObservedProgress>,
    loss: watch::Sender<bool>,
}

impl Fixture {
    async fn new(definition: Value) -> Self {
        use super::super::tests::{bootstrap, submission};
        let root = tempfile::tempdir().assert_value();
        let path = fs::canonicalize(root.path()).assert_value();
        let workspace = path.join("workspace");
        let state = path.join("state");
        let locks = path.join("locks");
        for directory in [&workspace, &state, &locks] {
            crate::execution::platform::create_private_directory(directory).assert_value();
        }
        let mut bootstrap = bootstrap(
            RunId::new("local-hook-attempt"),
            submission("local-hook-attempt"),
            workspace.clone(),
            state,
        );
        bootstrap.submission.environment = serde_json::from_value(definition).assert_value();
        let admitted = NativeV2Admission
            .admit(bootstrap.submission.clone())
            .await
            .assert_value();
        bootstrap.native_environment = BTreeMap::from([
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("LC_ALL".into(), "C".into()),
            ("LC_TEST_PRIVATE".into(), "unapproved".into()),
            ("UNDECLARED_TOKEN".into(), "unapproved".into()),
            ("ZEROSHOT_RUN_ID".into(), "invoking-run".into()),
        ]);
        let identity = WorkspaceIdentity::capture(&workspace).assert_value();
        let path = super::super::workspace_lease_path(&locks, &workspace);
        let lease = Arc::new(ControllerLease::acquire_workspace(&path, &workspace).assert_value());
        Self {
            _root: root,
            bootstrap,
            admitted,
            identity,
            lease,
            progress: Arc::new(ObservedProgress::default()),
            loss: watch::channel(false).0,
        }
    }

    fn hooks(&self) -> Arc<EnvironmentHooks> {
        EnvironmentHooks::for_run(&self.bootstrap, Some(&self.identity), Some(&self.lease))
            .assert_value()
    }

    fn request(&self, values: RunConnectionValues) -> CapsuleAllocationRequest<'_> {
        let mut request = allocation_request(&self.bootstrap.run_id, &self.admitted, None);
        request.preparation.environment =
            RunEnvironment::exact_for_admitted(&self.admitted, values).assert_value();
        request.preparation.progress = self.progress.clone();
        request
    }

    fn read(&self, file: &str) -> String {
        fs::read_to_string(self.bootstrap.workspace.join(file)).assert_value()
    }

    fn assert_log(&self, expected: &str) {
        let lines = self.progress.lines.lock().assert_value();
        assert!(lines.iter().any(|line| line == expected), "{lines:?}");
    }

    async fn wait_for_file(&self, name: &str) {
        let path = self.bootstrap.workspace.join(name);
        timeout(WAIT_LIMIT, async {
            while !fs::metadata(&path).is_ok_and(|metadata| metadata.len() != 0) {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .assert_value_with("hook wrote process marker");
    }
}

async fn assert_reaped(marker: &str) {
    let pid: libc::pid_t = marker.trim().parse().assert_value();
    assert!(pid > 1);
    timeout(WAIT_LIMIT, async {
        // Signal zero only checks whether the fixture's own process still exists.
        while unsafe { libc::kill(pid, 0) } == 0 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .assert_value_with("hook subprocess was reaped");
}

#[derive(Default)]
struct ObservedCleanup(Mutex<Vec<RunRuntimeExit>>);

#[async_trait]
impl CapsuleCleanup for ObservedCleanup {
    async fn destroy_or_confirm_absent(
        &self,
        exit: RunRuntimeExit,
    ) -> Result<CapsuleDestroyed, CapsuleCleanupUnavailable> {
        self.0.lock().assert_value().push(exit);
        Ok(CapsuleDestroyed::confirmed())
    }
}

#[tokio::test]
async fn local_hooks_share_explicit_values_and_release_services_after_teardown() {
    let fixture = Fixture::new(json!({
        "variables": {"PUBLIC_SETTING": "chosen", "ZEROSHOT_RUN_ID": "defined-run"},
        "connections": {"registry": ["REGISTRY_TOKEN"]},
        "setup": r#"
test "$PUBLIC_SETTING" = chosen
test "$ZEROSHOT_RUN_ID" = local-hook-attempt
test "$LC_ALL" = C
test -z "${LC_TEST_PRIVATE+x}"
test -z "${UNDECLARED_TOKEN+x}"
test -n "$REGISTRY_TOKEN"
printf 'setup\n' > order
printf 'registry %s\n' "$REGISTRY_TOKEN"
"#,
        "startup": "sleep 300 & printf '%s\\n' \"$!\" > service.pid\nprintf 'startup\\n' >> order",
        "teardown": "kill -0 \"$(cat service.pid)\"\nprintf 'teardown\\n' >> order"
    }))
    .await;
    let hooks = fixture.hooks();
    hooks.cleanup().await.assert_value();
    let values = serde_json::from_value(json!({"registry": {"REGISTRY_TOKEN": FIXTURE_SECRET}}))
        .assert_value();
    let request = fixture.request(values);
    hooks
        .prepare(&request, fixture.loss.subscribe())
        .await
        .assert_value();
    assert_eq!(fixture.read("order"), "setup\nstartup\n");
    assert_eq!(
        hooks.prepare(&request, fixture.loss.subscribe()).await,
        Err(CapsuleAllocationUnavailable::Runtime)
    );
    fixture.assert_log("[setup] registry [REDACTED]");
    assert!(
        fixture
            .progress
            .lines
            .lock()
            .assert_value()
            .iter()
            .all(|line| !line.contains(FIXTURE_SECRET))
    );

    let inner = Arc::new(ObservedCleanup::default());
    HookCleanup {
        hooks: hooks.clone(),
        inner: inner.clone(),
    }
    .destroy_or_confirm_absent(RunRuntimeExit::Completed)
    .await
    .assert_value();
    hooks.cleanup().await.assert_value();
    assert_eq!(*inner.0.lock().assert_value(), [RunRuntimeExit::Completed]);
    assert_eq!(fixture.read("order"), "setup\nstartup\nteardown\n");
    assert_reaped(&fixture.read("service.pid")).await;
}

#[tokio::test]
async fn preparation_failures_still_run_teardown_once_and_report_its_own_failure() {
    use CapsuleAllocationUnavailable::{EnvironmentSetup, EnvironmentStartup};
    let cases = [
        (
            "exit 21",
            "touch unexpected",
            EnvironmentSetup,
            "Environment setup failed with exit code 21",
        ),
        (
            "true",
            "exit 22",
            EnvironmentStartup,
            "Environment startup failed with exit code 22",
        ),
    ];
    for (setup, startup, reason, failed_phase) in cases {
        let fixture = Fixture::new(json!({
            "setup": setup,
            "startup": startup,
            "teardown": "printf 'cleanup\\n' >> order\nexit 23",
        }))
        .await;
        let hooks = fixture.hooks();
        assert_eq!(
            hooks
                .prepare(
                    &fixture.request(Default::default()),
                    fixture.loss.subscribe()
                )
                .await,
            Err(reason)
        );
        hooks.cleanup().await.assert_value();
        hooks.cleanup().await.assert_value();
        assert_eq!(fixture.read("order"), "cleanup\n");
        assert!(!fixture.bootstrap.workspace.join("unexpected").exists());
        fixture.assert_log(failed_phase);
        fixture.assert_log("Environment teardown failed with exit code 23");
    }
}

#[tokio::test]
async fn loss_and_explicit_stop_reap_unsettled_hooks_before_teardown() {
    use CapsuleAllocationUnavailable::{EnvironmentSetup, EnvironmentStartup};
    for (phase, reason) in [("setup", EnvironmentSetup), ("startup", EnvironmentStartup)] {
        let fixture = Fixture::new(json!({
            phase: "printf '%s\\n' \"$$\" > blocked.pid\nexec sleep 300",
            "teardown": "if kill -0 \"$(cat blocked.pid)\" 2>/dev/null; then exit 31; fi\nprintf 'done\\n' > order"
        }))
        .await;
        let hooks = fixture.hooks();
        let request = fixture.request(Default::default());
        let cancel = async {
            fixture.wait_for_file("blocked.pid").await;
            if phase == "setup" {
                fixture.loss.send_replace(true);
            } else {
                hooks.cleanup().await.assert_value();
            }
        };
        let (result, ()) = timeout(WAIT_LIMIT, async {
            tokio::join!(hooks.prepare(&request, fixture.loss.subscribe()), cancel)
        })
        .await
        .assert_value_with("cancelled hook settled");
        assert_eq!(result, Err(reason));
        hooks.cleanup().await.assert_value();
        assert_eq!(fixture.read("order"), "done\n");
        fixture.assert_log(&format!("Environment {phase} was cancelled"));
        assert_reaped(&fixture.read("blocked.pid")).await;
    }
}

#[tokio::test]
async fn hooks_recheck_workspace_and_control_after_start_logging() {
    for lose_control in [true, false] {
        let fixture = Fixture::new(json!({
            "startup": "touch unexpected",
            "teardown": "touch teardown",
        }))
        .await;
        let hooks = fixture.hooks();
        let control = fixture.loss.clone();
        let lease = fixture.lease.path().to_owned();
        *fixture.progress.action.lock().assert_value() = Some((
            "Environment startup started",
            Box::new(move || {
                if lose_control {
                    control.send_replace(true);
                } else {
                    fs::rename(&lease, lease.with_extension("replaced")).assert_value();
                }
            }),
        ));
        assert_eq!(
            hooks
                .prepare(
                    &fixture.request(Default::default()),
                    fixture.loss.subscribe()
                )
                .await,
            Err(CapsuleAllocationUnavailable::EnvironmentStartup)
        );
        hooks.cleanup().await.assert_value();
        assert!(!fixture.bootstrap.workspace.join("unexpected").exists());
        assert_eq!(
            fixture.bootstrap.workspace.join("teardown").exists(),
            lose_control
        );
        if lose_control {
            fixture.assert_log("Environment startup was cancelled");
        } else {
            fixture.assert_log(
                "Environment startup could not start because the run workspace became unavailable",
            );
            fixture.assert_log(
                "Environment teardown failed because the run workspace became unavailable",
            );
        }
    }
}

#[tokio::test]
async fn draft_or_blank_hooks_and_unavailable_preparation_connections_do_not_start_work() {
    for definition in [
        Value::Null,
        json!({"setup": " ", "startup": "", "teardown": "\n"}),
    ] {
        let fixture = Fixture::new(definition).await;
        let environment = fixture.bootstrap.submission.environment.as_ref();
        assert!(!has_local_teardown(environment));
        assert!(
            EnvironmentHooks::for_run(
                &fixture.bootstrap,
                Some(&fixture.identity),
                Some(&fixture.lease)
            )
            .is_none()
        );
    }
    let fixture = Fixture::new(json!({
        "connections": {"registry": ["REGISTRY_TOKEN"]},
        "setup": "touch unexpected",
        "teardown": "touch unexpected",
    }))
    .await;
    assert!(has_local_teardown(
        fixture.bootstrap.submission.environment.as_ref()
    ));
    assert!(EnvironmentHooks::for_run(&fixture.bootstrap, None, Some(&fixture.lease)).is_none());
    assert!(EnvironmentHooks::for_run(&fixture.bootstrap, Some(&fixture.identity), None).is_none());
    let hooks = fixture.hooks();
    let request = allocation_request(&fixture.bootstrap.run_id, &fixture.admitted, None);
    assert_eq!(
        hooks.prepare(&request, fixture.loss.subscribe()).await,
        Err(CapsuleAllocationUnavailable::Runtime)
    );
    hooks.cleanup().await.assert_value();
    assert!(hooks.job.get().is_none());
    assert!(!fixture.bootstrap.workspace.join("unexpected").exists());
}
