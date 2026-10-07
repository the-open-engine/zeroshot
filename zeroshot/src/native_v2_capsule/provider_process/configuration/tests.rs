use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use openengine_cluster_protocol::WorkerOutcome;
use openengine_cluster_testkit::assertions::{AssertAt, AssertValue};
use serde_json::json;

use super::*;
use crate::execution::WorkspaceAccessMode;
use crate::execution::driver::WorkspaceCapability;
#[cfg(target_os = "linux")]
use crate::execution::process::HostedProcessPool;
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_capsule::provider_process::{
    ProviderExecution, ProviderFilesystemConfig, ProviderProcessRunners, ProviderSessionCore,
    impl_provider_node_session,
};
use crate::native_v2_contract::NodeInvocation;
use crate::native_v2_runner::{
    DriverInvocation, NativeNodeRunner, NodeDriver, NodeHandle, NodeRole, NodeRunner, NodeSession,
    ResolvedEnvironment, SessionFactory,
};
use crate::native_v2_runner::test_support::{admitted, request};

struct InspectionSession {
    core: ProviderSessionCore,
}

impl_provider_node_session!(InspectionSession);

struct Fixture {
    _directory: TestDirectory,
    driver: Arc<InspectionDriver>,
}

struct InspectionDriver {
    workspace: PathBuf,
    runtime: PathBuf,
    script: String,
    runners: ProviderProcessRunners,
    rounds: usize,
    results: Mutex<Vec<Option<Vec<Value>>>>,
    idle_after_inspection: AtomicBool,
}

impl Fixture {
    fn new(script: &str) -> Self {
        let directory = TestDirectory::new("configuration-inspection");
        let workspace = directory.child("workspace");
        let runtime = directory.child("runtime");
        fs::create_dir(&workspace).assert_value();
        fs::create_dir(&runtime).assert_value();
        Self {
            _directory: directory,
            driver: Arc::new(InspectionDriver {
                workspace,
                runtime,
                script: script.to_owned(),
                runners: ProviderProcessRunners::local(),
                rounds: 1,
                results: Mutex::new(Vec::new()),
                idle_after_inspection: AtomicBool::new(false),
            }),
        }
    }

    async fn start(&self, node: &str) -> NodeHandle {
        NativeNodeRunner::new(&admitted(), self.driver.clone(), self.driver.clone())
            .assert_value()
            .start(request("inspection", node, (1, 1)))
            .await
            .assert_value()
    }

    fn assert_cleaned(&self) {
        assert!(self.driver.idle_after_inspection.load(Ordering::SeqCst));
        assert!(!self.driver.runtime.join("execution-1").exists());
    }
}

#[async_trait]
impl SessionFactory for InspectionDriver {
    async fn open(
        &self,
        _invocation: &NodeInvocation,
        _environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        Ok(Arc::new(InspectionSession {
            core: ProviderSessionCore::new(),
        }))
    }
}

#[async_trait]
impl NodeDriver for InspectionDriver {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let session = invocation
            .session
            .as_any()
            .downcast_ref::<InspectionSession>()
            .assert_value();
        let execution = ProviderExecution::new(
            ProviderFilesystemConfig {
                runners: self.runners,
                root: &self.runtime,
                workspace: &self.workspace,
            },
            &invocation,
            &session.core,
        );
        let files = execution.prepare(&control).await.assert_value();
        for _ in 0..self.rounds {
            let result =
                inspect_configuration(files.clone(), self.command(&files), &control, requests())
                    .await;
            // prepare refuses reuse while any previous contained process lacks cleanup evidence.
            execution.prepare(&control).await.assert_value();
            let pid_file = files.workspace.join("child-pid");
            if pid_file.exists() {
                assert!(
                    !process_is_live(read_pid(&pid_file)),
                    "probe descendant survived inspection"
                );
            }
            self.idle_after_inspection.store(true, Ordering::SeqCst);
            self.results.lock().assert_value().push(result?);
        }
        let response = if invocation.role == NodeRole::Verifier {
            r#"{"output":null,"signals":{},"diagnostic":null}"#
        } else {
            "null"
        };
        Ok(invocation
            .response
            .parse_agent_response(response)
            .assert_value())
    }
}

impl InspectionDriver {
    fn command(&self, files: &ProviderExecutionFiles) -> ProcessSessionCommand {
        ProcessSessionCommand {
            program: "/bin/sh".to_owned(),
            argv: vec!["-c".to_owned(), self.script.clone()],
            environment: BTreeMap::from([
                ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
                ("CANDIDATE".to_owned(), self.workspace.display().to_string()),
            ]),
            workspace: WorkspaceCapability {
                current_dir: files.workspace.clone(),
                mode: WorkspaceAccessMode::ReadWrite,
            },
            deadline: None,
        }
    }
}

fn requests() -> Vec<ConfigurationRequest> {
    (1..=2)
        .map(|id| ConfigurationRequest {
            messages: vec![json!({"id":id,"method":"configuration"})],
            response_pointer: "/id",
            response_id: json!(id),
        })
        .collect()
}

const SEQUENTIAL: &str = r#"set -eu
IFS= read -r first
test "$first" = '{"id":1,"method":"configuration"}'
printf '%s\n' '{"notification":"ignore this"}' '{"id":1,"result":{"policy":null}}'
IFS= read -r second
test "$second" = '{"id":2,"method":"configuration"}'
# The final record deliberately lacks a newline; EOF completes it.
printf '%s' '{"id":2,"result":{"requirements":null}}'
"#;

#[tokio::test]
async fn sequential_requests_keep_stdin_open_and_reuse_files_after_cleanup() {
    let mut fixture = Fixture::new(SEQUENTIAL);
    Arc::get_mut(&mut fixture.driver).assert_value().rounds = 2;
    fixture
        .start("worker")
        .await
        .completion()
        .await
        .assert_value();
    let results = fixture.driver.results.lock().assert_value();
    let expected = Some(vec![
        json!({"id":1,"result":{"policy":null}}),
        json!({"id":2,"result":{"requirements":null}}),
    ]);
    assert_eq!(*results, [expected.clone(), expected]);
    fixture.assert_cleaned();
}

#[tokio::test]
async fn malformed_and_oversized_configuration_is_private_and_does_not_block_reuse() {
    for script in [
        "printf '%s\\n' 'private-credential malformed-json'; printf '%s\\n' 'private-stderr' >&2; cat >/dev/null",
        "head -c 4194305 /dev/zero; cat >/dev/null",
    ] {
        let fixture = Fixture::new(script);
        let mut handle = fixture.start("worker").await;
        let mut output = handle.take_initial_output().assert_value();
        handle.completion().await.assert_value();
        assert!(
            output.recv().await.is_err(),
            "configuration escaped through durable output"
        );
        assert_eq!(*fixture.driver.results.lock().assert_value(), [None]);
        fixture.assert_cleaned();
    }
}

const HANGING: &str = "sleep 30 & child=$!; printf '%s\\n' \"$child\" > child-pid; wait";

#[tokio::test]
async fn cancellation_reaps_descendants_before_execution_files_become_reusable() {
    let fixture = Fixture::new(HANGING);
    let mut handle = fixture.start("worker").await;
    wait_for_pid(&fixture.driver.workspace.join("child-pid")).await;
    handle.cancel();
    assert_eq!(handle.completion().await, Err(NodeRunnerError::Cancelled));
    fixture.assert_cleaned();
}

#[tokio::test]
async fn hung_configuration_query_expires_and_reaps_its_descendants() {
    let fixture = Fixture::new(HANGING);
    let started = Instant::now();
    timeout(
        Duration::from_secs(20),
        fixture.start("worker").await.completion(),
    )
    .await
    .assert_value()
    .assert_value();
    assert!(started.elapsed() >= INSPECTION_BUDGET);
    assert_eq!(*fixture.driver.results.lock().assert_value(), [None]);
    fixture.assert_cleaned();
}

async fn wait_for_pid(path: &Path) {
    timeout(Duration::from_secs(5), async {
        while fs::read_to_string(path)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .assert_value();
}

fn read_pid(path: &Path) -> u32 {
    fs::read_to_string(path)
        .assert_value()
        .trim()
        .parse()
        .assert_value()
}

fn process_is_live(pid: u32) -> bool {
    // SAFETY: signal zero inspects existence; it does not deliver a signal or mutate the process.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn root_hosted_inspection_uses_shared_workspace_and_cleans_its_command_domain() {
    // SAFETY: geteuid only reads this process's identity.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only hosted shared workspace gate skipped");
        return;
    }
    let mut fixture = Fixture::new(&format!(
        r#"set -eu
touch "$CANDIDATE/shared-write"
printf inspected > probe-write
{SEQUENTIAL}"#
    ));
    let pool = HostedProcessPool::new(111_002, 111_002, 112_000).assert_value();
    crate::native_v2_capsule::prepare_capsule_filesystem(
        crate::native_v2_capsule::CapsuleFilesystemSpec {
            workspace: &fixture.driver.workspace,
            runtime_home: &fixture.driver.runtime,
            process_pool: pool,
        },
    )
    .assert_value();
    Arc::get_mut(&mut fixture.driver).assert_value().runners = ProviderProcessRunners::hosted(pool);
    fixture
        .start("verify")
        .await
        .completion()
        .await
        .assert_value();
    assert!(
        fixture
            .driver
            .results
            .lock()
            .assert_value()
            .assert_at(0)
            .is_some()
    );
    assert!(fixture.driver.workspace.join("probe-write").exists());
    assert!(fixture.driver.workspace.join("shared-write").exists());
    fixture.assert_cleaned();
}
