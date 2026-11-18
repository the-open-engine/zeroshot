use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::active_memory::hooks::{ActiveMemoryHooks, ActiveMemorySource, HookQuery};
use crate::v2_run_ledger::{CreateRun, RunEvent, RunLedger, SafeLogStream};
use openengine_cluster_protocol::{RunId, Sha256Digest};

struct Source {
    context: String,
    enabled: bool,
    queries: AtomicUsize,
}

impl ActiveMemorySource for Source {
    fn enabled(&self) -> bool {
        self.enabled
    }
    fn tool_context(&self, _: &HookQuery) -> String {
        self.queries.fetch_add(1, Ordering::Relaxed);
        self.context.clone()
    }
    fn graph_context(&self, _: &str, _: &str) -> String {
        String::new()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backpressured_memory_handoffs_stay_silent_without_failing_the_turn() {
    handoffs("memory_handoff_delayed", true, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acknowledged_memory_handoffs_stay_silent() {
    handoffs("memory_handoff", true, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_memory_replies_stay_silent() {
    handoffs("memory_handoff_delayed", false, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_memory_replies_stay_silent() {
    handoffs("memory_handoff_delayed", false, false).await;
}

async fn handoffs(mode: &str, nonempty: bool, enabled: bool) {
    let directory = TestDirectory::new("copilot-memory");
    let executable = directory.write_executable("copilot", &script());
    let mut values = fixture_values(&directory, mode);
    values.insert(auth::TOKEN.to_owned(), "gho_fake-secret".into());
    let binding = binding(values.keys().cloned());
    let admitted = admitted(binding.clone(), INSTRUCTIONS).await;
    let run = RunId::new("run-copilot");
    let ledger = Arc::new(crate::v2_run_ledger::fake::FakeRunLedger::new());
    ledger
        .create_or_get(CreateRun {
            run_id: run.clone(),
            submission_key: IdempotencyKey::new("copilot-memory").assert_value(),
            submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
            admitted: admitted.clone(),
        })
        .await
        .assert_value();
    let source = Arc::new(Source {
        // Larger than the peer's pipe, within the normal repository recall budget.
        context: if nonempty {
            "[lesson@1] ".repeat(2500)
        } else {
            String::new()
        },
        enabled,
        queries: AtomicUsize::new(0),
    });
    let hooks = Arc::new(ActiveMemoryHooks::new(source.clone()).assert_value());
    hooks.bind_ledger(run.clone(), ledger.clone());
    let workspace = directory.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    crate::execution::platform::private_directory(&directory.child("runtime")).assert_value();
    let adapter = Arc::new(
        CopilotAdapter::new_local(CopilotConfig {
            executable,
            workspace,
            runtime_home: directory.child("runtime"),
            local_user: None,
            base_environment: BTreeMap::new(),
            local_command_environment: BTreeMap::new(),
            search_path: "/usr/bin:/bin".into(),
            process_pool: HostedProcessPool::hosted_default(),
        })
        .with_active_memory(Some(hooks.clone())),
    );
    let runtime = NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value();
    let fixture = Fixture {
        directory,
        runtime,
        binding,
        instructions: INSTRUCTIONS,
        values,
    };
    verified(complete(fixture.start(1).await).await.1);
    // Local hosting retires delivery after the turn settles; repeated retirement stays quiet.
    hooks.finish_steers(1, true).await;
    hooks.finish_steers(1, true).await;
    let replies: Vec<Value> =
        serde_json::from_str(&fixture.directory.read("capture.memory-replies")).assert_value();
    assert_eq!(replies.len(), 2);
    for reply in replies {
        if nonempty {
            assert_eq!(
                reply["result"]["output"]["additionalContext"],
                source.context
            );
        } else {
            assert!(reply["result"]["output"].is_null());
        }
    }
    assert_eq!(source.queries.load(Ordering::Relaxed), 2);
    let events = ledger
        .snapshot_and_tail(&run, None)
        .await
        .assert_value()
        .events;
    let errors = events
        .iter()
        .filter_map(|stored| match &stored.event {
            RunEvent::SafeLog {
                execution,
                stream: SafeLogStream::Error,
                diagnostic,
                line,
                ..
            } => {
                assert!(execution.is_none());
                assert!(
                    diagnostic.is_none(),
                    "memory must not become the primary failure"
                );
                Some(line.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(errors.is_empty(), "{errors:?}");
    fixture.runtime.close_run(&run).await;
}
