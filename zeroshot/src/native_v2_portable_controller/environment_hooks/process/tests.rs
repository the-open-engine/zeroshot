use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use openengine_cluster_testkit::fixture::TemporaryDirectory;

use super::*;
use crate::execution::platform::process_exists;
use crate::native_v2_platform::PreparationProgress;

const TEST_BUDGET: Duration = Duration::from_secs(10);

struct Fixture {
    directory: TemporaryDirectory,
    shell: HookShell,
    progress: Arc<RecordedLines>,
}

impl Fixture {
    fn new() -> Self {
        let directory = TemporaryDirectory::for_test("local-hook-process");
        let workspace = directory.path("workspace");
        std::fs::create_dir(&workspace).assert_value();
        Self {
            shell: HookShell {
                workspace,
                readiness: directory.path("startup-status"),
                environment: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
            },
            directory,
            progress: Arc::new(RecordedLines::default()),
        }
    }

    async fn launch(&self, script: &str, timeout: Option<Duration>) -> HookProcess {
        self.shell
            .launch(
                script,
                timeout.map(|timeout| Instant::now() + timeout),
                HookOutput::new(
                    self.progress.clone(),
                    vec!["test-private-value".to_owned()],
                    "hook",
                ),
            )
            .await
            .assert_value()
    }

    fn service_pid(&self) -> u32 {
        std::fs::read_to_string(self.directory.path("workspace/pid"))
            .assert_value()
            .trim()
            .parse()
            .assert_value()
    }
}

#[derive(Default)]
struct RecordedLines(Mutex<Vec<String>>);

#[async_trait]
impl PreparationProgress for RecordedLines {
    async fn log(&self, line: &str) -> Result<(), CapsuleAllocationUnavailable> {
        self.0.lock().assert_value().push(line.into());
        Ok(())
    }
}

#[tokio::test]
async fn startup_reports_bash_status_and_retains_background_children_through_failure() {
    for (preparation, expected) in [(":", 0), ("exit 27", 27)] {
        let fixture = Fixture::new();
        let script = format!(
            "if read -r unexpected; then exit 41; fi\n/bin/sleep 300 &\nprintf '%s\\n' \"$!\" > pid\n{preparation}"
        );
        let mut process = fixture.launch(&script, None).await;
        let status = tokio::time::timeout(
            TEST_BUDGET,
            process.startup_status(&fixture.shell.readiness, HookStop::default()),
        )
        .await
        .assert_value();
        assert!(matches!(status, StartupStatus::Reported(code) if code == expected));
        let pid = fixture.service_pid();
        assert!(process_exists(pid).assert_value());

        process.cancellation().send_replace(true);
        assert!(matches!(
            process.finish().await.assert_value(),
            HookExit::Interrupted("was cancelled")
        ));
        assert!(!process_exists(pid).assert_value());
    }
}

#[tokio::test]
async fn startup_exits_without_status_when_the_custodian_cannot_publish_readiness() {
    let mut fixture = Fixture::new();
    let path = fixture.directory.path("missing-parent/startup-status");
    fixture.shell.readiness = path;
    let mut process = fixture.launch(":", None).await;
    let status = tokio::time::timeout(
        TEST_BUDGET,
        process.startup_status(&fixture.shell.readiness, HookStop::default()),
    )
    .await
    .assert_value();
    assert_eq!(status.detail(), "exited without a startup status");
    assert!(matches!(status, StartupStatus::Missing));
    assert!(matches!(
        process.finish().await.assert_value(),
        HookExit::Code(125)
    ));
}

#[tokio::test]
async fn startup_observes_read_errors_and_cancellation_during_readiness() {
    let fixture = Fixture::new();
    // A directory at the status path makes the read fail without delaying the real shell.
    std::fs::create_dir(&fixture.shell.readiness).assert_value();
    let mut process = fixture.launch("exec /bin/sleep 300", None).await;
    let status = process
        .startup_status(&fixture.shell.readiness, HookStop::default())
        .await;
    assert!(matches!(status, StartupStatus::Missing));

    let (sender, stopping) = watch::channel(false);
    sender.send_replace(true);
    let status = process
        .startup_status(
            &fixture.shell.readiness,
            HookStop {
                stopping: Some(stopping),
                lost: None,
            },
        )
        .await;
    assert!(matches!(status, StartupStatus::Stopped));
    assert_eq!(status.detail(), "was cancelled");
    process.stop();
    process.finish().await.assert_value();
}

#[tokio::test]
async fn startup_readiness_wait_expires_without_giving_the_service_a_process_deadline() {
    let fixture = Fixture::new();
    let mut process = fixture.launch("exec /bin/sleep 300", None).await;
    tokio::time::pause();
    let (status, ()) = tokio::join!(
        process.startup_status(&fixture.shell.readiness, HookStop::default()),
        tokio::time::advance(PREPARATION_PHASE_TIMEOUT),
    );
    tokio::time::resume();
    assert!(matches!(status, StartupStatus::TimedOut));
    assert_eq!(status.detail(), "timed out");
    process.stop();
    assert!(matches!(
        process.finish().await.assert_value(),
        HookExit::Interrupted("was cancelled")
    ));
}

#[test]
fn startup_status_record_requires_one_bounded_exit_code_and_a_newline() {
    let fixture = Fixture::new();
    let path = &fixture.shell.readiness;
    assert_eq!(read_startup_status(path).assert_value(), None);
    for (record, expected) in [
        ("", None),
        ("0", None),
        ("0\n", Some(0)),
        ("255\n", Some(255)),
        ("256\n", None),
        ("1\n0\n", None),
        ("1\nlarge remainder", None),
    ] {
        std::fs::write(path, record).assert_value();
        assert_eq!(
            read_startup_status(path).assert_value(),
            expected,
            "{record:?}"
        );
    }
    std::fs::write(path, [0xff, b'\n']).assert_value();
    assert_eq!(
        read_startup_status(path).assert_error().kind(),
        io::ErrorKind::InvalidData
    );
}

#[tokio::test]
async fn startup_rejects_a_non_utf8_status_path_before_launch() {
    let mut fixture = Fixture::new();
    fixture
        .shell
        .readiness
        .set_file_name(OsStr::from_bytes(b"status-\xff"));
    let result = fixture
        .shell
        .launch(
            "touch unexpected",
            None,
            HookOutput::new(fixture.progress, vec![], "startup"),
        )
        .await;
    assert!(matches!(result, Err(ProcessRunnerError::InvalidCommand(_))));
    assert!(!fixture.directory.path("workspace/unexpected").exists());
}

#[tokio::test]
async fn finite_hooks_close_input_redact_output_and_enforce_pipefail() {
    let fixture = Fixture::new();
    let process = fixture
        .launch(
            r#"
if read -r unexpected; then exit 31; fi
printf 'before test-private-value\n'
printf 'from stderr\n' >&2
printf 'without newline'
false | true
exit 32
"#,
            Some(TEST_BUDGET),
        )
        .await;
    assert!(matches!(
        process.finish().await.assert_value(),
        HookExit::Code(1)
    ));
    assert_eq!(
        *fixture.progress.0.lock().assert_value(),
        [
            "[hook] before [REDACTED]",
            "[hook] from stderr",
            "[hook] without newline"
        ]
    );
}

#[tokio::test]
async fn finite_hooks_distinguish_deadlines_from_shell_signals() {
    let fixture = Fixture::new();
    let timed_out = fixture
        .launch("exec /bin/sleep 300", Some(Duration::from_millis(100)))
        .await;
    assert!(matches!(
        timed_out.finish().await.assert_value(),
        HookExit::Interrupted("timed out")
    ));

    let signalled = fixture.launch("kill -TERM \"$$\"", Some(TEST_BUDGET)).await;
    assert!(matches!(
        signalled.finish().await.assert_value(),
        HookExit::Interrupted("terminated before completing")
    ));
}

enum BlockedProgress {
    Reject,
    Hang,
}

#[async_trait]
impl PreparationProgress for BlockedProgress {
    async fn log(&self, _: &str) -> Result<(), CapsuleAllocationUnavailable> {
        match self {
            Self::Reject => Err(CapsuleAllocationUnavailable::Runtime),
            Self::Hang => std::future::pending().await,
        }
    }
}

#[tokio::test]
async fn hook_output_failure_drains_following_chunks_and_preserves_process_cleanup() {
    let fixture = Fixture::new();
    let process = fixture
        .shell
        .launch(
            "for ((n=0; n<80; n++)); do printf '%8192s\\n' x; done\ntouch completed",
            Some(Instant::now() + TEST_BUDGET),
            HookOutput::new(Arc::new(BlockedProgress::Reject), vec![], "setup"),
        )
        .await
        .assert_value();
    assert!(matches!(
        process.finish().await.assert_value(),
        HookExit::Interrupted("failed while collecting process output")
    ));
    assert!(fixture.directory.path("workspace/completed").is_file());
}

#[tokio::test]
async fn hook_output_wait_is_bounded_after_the_process_settles() {
    let fixture = Fixture::new();
    let mut process = fixture
        .shell
        .launch(
            "printf 'output\\n'",
            Some(Instant::now() + TEST_BUDGET),
            HookOutput::new(Arc::new(BlockedProgress::Hang), vec![], "teardown"),
        )
        .await
        .assert_value();
    process.session.wait().await.assert_value();

    tokio::time::pause();
    let (exit, ()) = tokio::join!(
        process.finish(),
        tokio::time::advance(Duration::from_secs(5))
    );
    tokio::time::resume();
    assert!(matches!(
        exit.assert_value(),
        HookExit::Interrupted("failed while collecting process output")
    ));
}
