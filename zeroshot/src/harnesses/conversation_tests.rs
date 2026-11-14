//! Shared stub-provider regression, invoked beside every adapter's file layout.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use openengine_cluster_protocol::{NodeName, PayloadType};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::{HarnessAdapter, HarnessBuildContext, HarnessHost, HarnessRegistry};
use crate::native_v2_candidate::test_support::{NodeRequestFixture, TestDirectory};
use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;
use crate::native_v2_contract::{NodeInstanceId, NodeRuntimeBinding, RuntimeLane};
use crate::native_v2_runner::{
    DriverInvocation, DurableNodeEvent, NodeResponseContract, NodeRole, NodeSession,
};
use crate::native_v2_runner::conversations::{ConversationSources, capture};

const CONTENT: &str = "first-turn-content-only-in-provider-files";
const SECRET: &str = "credential-and-configuration-marker";

fn adapter(root: &Path, executable: &Path, harness: &str, local: bool) -> HarnessAdapter {
    for name in ["workspace", "runtime", "user", "provider"] {
        fs::create_dir_all(root.join(name)).assert_value();
    }
    let native = if local {
        BTreeMap::from([
            (
                "CODEX_HOME".to_owned(),
                root.join("provider").display().to_string(),
            ),
            (
                "COPILOT_HOME".to_owned(),
                root.join("provider").display().to_string(),
            ),
            (
                "CLAUDE_CONFIG_DIR".to_owned(),
                root.join("provider").display().to_string(),
            ),
        ])
    } else {
        BTreeMap::new()
    };
    let provider = if harness == "claude" {
        "anthropic"
    } else if harness == "copilot" {
        "github"
    } else {
        "openai"
    };
    HarnessRegistry::local()
        .configure(HarnessBuildContext {
            lane: RuntimeLane::new(harness, provider).assert_value(),
            workspace: root.join("workspace"),
            runtime_home: root.join("runtime"),
            executable: executable.to_owned(),
            prefix_arguments: Vec::new(),
            search_path: "/usr/bin:/bin".to_owned(),
            base_environment: BTreeMap::new(),
            host: HarnessHost::Local {
                home: local.then(|| root.join("user")),
                environment: LocalHarnessEnvironment::new(native),
            },
        })
        .assert_value()
        .build(None)
        .assert_value()
}

fn invocation(session: Arc<dyn NodeSession>, successor: bool, harness: &str) -> DriverInvocation {
    let key = match harness {
        "codex" => Some("OPENAI_API_KEY"),
        "copilot" => Some("COPILOT_GITHUB_TOKEN"),
        _ => None,
    };
    let binding: NodeRuntimeBinding = serde_json::from_value(
        json!({"kind":"agent","model":"auto","sessionScope":"node_instance",
               "connections": key.map(|key| json!({"provider":[key]})).unwrap_or(json!({}))}),
    )
    .assert_value();
    let request = NodeRequestFixture {
        run_id: if successor {
            "successor"
        } else {
            "predecessor"
        },
        node: "worker",
        node_instance: 1,
        execution: if successor { 2 } else { 1 },
        worker: "agent.worker@1",
        instructions: "Return null.",
        input: Value::Null,
        binding,
        environment: key
            .into_iter()
            .map(|key| {
                (
                    crate::native_v2_candidate::test_support::environment_name(key),
                    "checkpoint-fixture-token".to_owned(),
                )
            })
            .collect(),
    }
    .into_request();
    DriverInvocation {
        dispatch: Default::default(),
        host_context: None,
        node: request.invocation,
        role: NodeRole::Worker,
        response: NodeResponseContract::Worker {
            output: PayloadType::Null,
        },
        environment: request.environment,
        session,
        provider_session_slot: NodeInstanceId::new(1).assert_value(),
        provider_home_slot: NodeInstanceId::new(if successor { 8 } else { 1 }).assert_value(),
        auxiliary: false,
    }
}

async fn turn(adapter: &HarnessAdapter, invocation: DriverInvocation) -> Vec<DurableNodeEvent> {
    let (_cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Ok(event) = durable.recv().await {
            events.push(event);
        }
        events
    });
    let outcome = adapter.driver.run(invocation, control).await;
    let events = drain.await.assert_value();
    let outcome = outcome
        .unwrap_or_else(|error| panic!("provider turn failed: {error:?}; events: {events:?}"));
    assert!(matches!(
        outcome,
        openengine_cluster_protocol::WorkerOutcome::Verified { .. }
    ));
    events
}

pub(crate) async fn exercise(harness: &str) {
    for local in [false, true] {
        let directory = TestDirectory::new("checkpoint-provider-roundtrip");
        let script =
            include_str!("conversation_tests/provider.py").replace("HARNESS_NAME", harness);
        let executable = directory.write_executable(
            "provider",
            &openengine_cluster_testkit::fixture::python3_script(&script),
        );
        let first = directory.child("first");
        let adapter_one = adapter(&first, &executable, harness, local);
        // Open through the adapter, then let the stub provider write its native session file.
        let dummy = Arc::new(Unopened);
        let prototype = invocation(dummy, false, harness);
        let session = adapter_one
            .sessions
            .open(&prototype.node, &prototype.environment)
            .await
            .assert_value();
        turn(&adapter_one, invocation(session.clone(), false, harness)).await;
        let source = session.conversation().await;
        if harness == "opencode" && local {
            assert!(
                source.is_err(),
                "a local shared OpenCode database must not be copied"
            );
            session.close().await;
            continue;
        }
        let source = source.unwrap_or_else(|reason| panic!("{harness} capture: {reason}"));
        let id = source.metadata.id.clone();
        assert!(!id.is_empty());
        assert!(
            source.storage.is_dir(),
            "{harness} storage {} missing; addressed {}",
            source.storage.display(),
            fs::read_to_string(first.join("workspace/address.json")).assert_value()
        );
        fs::write(source.storage.join("auth.json"), SECRET).assert_value();
        fs::write(first.join("user/config.json"), SECRET).assert_value();
        fs::write(first.join("runtime/provider-credentials"), SECRET).assert_value();
        let node = NodeName::new("worker").assert_value();
        let captured = capture(ConversationSources::from([(
            NodeInstanceId::new(1).assert_value(),
            (node.clone(), Ok(source)),
        )]));
        assert!(captured.notices.is_empty());
        let files = captured.files.assert_value();
        fn no_secret(path: &Path) {
            for entry in fs::read_dir(path).assert_value() {
                let path = entry.assert_value().path();
                if path.is_dir() {
                    no_secret(&path);
                } else {
                    assert!(
                        !fs::read(path)
                            .assert_value()
                            .windows(SECRET.len())
                            .any(|value| value == SECRET.as_bytes())
                    );
                }
            }
        }
        no_secret(files.directory());
        session.close().await;
        assert!(!first.join("runtime/writer-node-instance-1").exists());
        fs::remove_dir_all(&first).assert_value();
        let second = directory.child("second");
        let adapter_two = adapter(&second, &executable, harness, local);
        fs::write(second.join("workspace/expected-id"), &id).assert_value();
        let restored = files
            .conversation(NodeInstanceId::new(1).assert_value(), &node)
            .assert_value();
        let session = adapter_two
            .sessions
            .open(&prototype.node, &prototype.environment)
            .await
            .assert_value();
        assert!(session.restore_conversation(restored).await);
        let events = turn(&adapter_two, invocation(session.clone(), true, harness)).await;
        let addressed: Value =
            serde_json::from_slice(&fs::read(second.join("workspace/address.json")).assert_value())
                .assert_value();
        assert_eq!(addressed["id"], id);
        assert_eq!(addressed["content"], CONTENT);
        if harness == "codex" {
            let usage = events
                .iter()
                .find_map(|event| match event {
                    DurableNodeEvent::TokenUsage(Some(usage)) => Some(usage),
                    _ => None,
                })
                .assert_value();
            assert_eq!(usage.input_tokens.get(), 30);
            assert_eq!(usage.output_tokens.get(), 7);
        }
        session.close().await;
    }
}

pub(crate) async fn codex_requires_current_usage() {
    for stale in [false, true] {
        let directory = TestDirectory::new("codex-checkpoint-missing-usage");
        let script =
            include_str!("conversation_tests/provider.py").replace("HARNESS_NAME", "codex");
        let executable = directory.write_executable(
            "provider",
            &openengine_cluster_testkit::fixture::python3_script(&script),
        );
        let first = directory.child("first");
        let adapter = adapter(&first, &executable, "codex", true);
        if !stale {
            fs::write(first.join("workspace/omit-usage"), "").assert_value();
        }
        let prototype = invocation(Arc::new(Unopened), false, "codex");
        let session = adapter
            .sessions
            .open(&prototype.node, &prototype.environment)
            .await
            .assert_value();
        turn(&adapter, invocation(session.clone(), false, "codex")).await;
        if stale {
            let source = session.conversation().await.assert_value();
            assert_eq!(source.metadata.usage.assert_value().input_tokens.get(), 100);
            fs::write(first.join("workspace/expected-id"), &source.metadata.id).assert_value();
            fs::write(first.join("workspace/omit-usage"), "").assert_value();
            turn(&adapter, invocation(session.clone(), false, "codex")).await;
        }
        assert_eq!(
            session.conversation().await.err(),
            Some("Codex cumulative usage baseline is unavailable for the checkpoint turn")
        );
        session.close().await;
    }
}

struct Unopened;
#[async_trait::async_trait]
impl NodeSession for Unopened {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn is_live(&self) -> bool {
        true
    }
    async fn close(&self) {}
}

#[tokio::test]
async fn core_checkpoint_conversations_move_between_empty_homes_and_workspaces() {
    for harness in ["codex", "claude", "pi", "copilot"] {
        exercise(harness).await;
    }
}
