use super::*;
use crate::native_v2_runner::{DriverInvocation, NodeResponseContract, NodeRole};
use crate::native_v2_runner::conversations::{capture, ConversationSources};
use crate::native_v2_contract::NodeInstanceId;
use openengine_cluster_protocol::PayloadType;

fn isolated(settings: &mut LiveSettings, root: &std::path::Path, workspace: PathBuf) {
    settings.workspace = workspace;
    for (name, leaf) in [
        ("HOME", "user"),
        ("CODEX_HOME", "provider"),
        ("CLAUDE_CONFIG_DIR", "provider"),
        ("COPILOT_HOME", "provider"),
    ] {
        let path = root.join(leaf);
        std::fs::create_dir_all(&path).assert_value();
        settings
            .environment
            .insert(name.to_owned(), path.display().to_string());
    }
}

fn configured_adapter(settings: &LiveSettings, runtime: PathBuf) -> super::super::HarnessAdapter {
    let registry = HarnessRegistry::local();
    let mut context = settings.context(&registry, runtime);
    if settings.lane.harness_name() == "opencode" {
        // Exercise the exclusively owned storage layout without requiring a root capsule.
        context.host = HarnessHost::Local {
            home: None,
            environment: LocalHarnessEnvironment::new(settings.environment.clone()),
        };
    }
    registry
        .configure(context)
        .assert_value()
        .build(None)
        .assert_value()
}

async fn turn(
    adapter: &super::super::HarnessAdapter,
    settings: &LiveSettings,
    next: bool,
    restored: Option<crate::native_v2_runner::conversations::ConversationSource>,
) -> (Arc<dyn crate::native_v2_runner::NodeSession>, Value) {
    let binding = settings.binding(&HarnessRegistry::local(), SessionScope::NodeInstance);
    let instructions = if next {
        "Return the exact secret word you learned in the preceding turn of this provider conversation. The source file is gone. Do not inspect any other files or use network tools. Obey the runtime response contract without encoding another JSON object inside the output string."
    } else {
        "Use native tools to run `cat secret-word && rm -- secret-word && test ! -e secret-word`. Remember the printed word in this provider conversation. Unlink the file completely; truncating it is insufficient. Never save the word in another workspace file. Return the literal string \"stored\" as output. Do not use Git or network tools. Obey the runtime response contract without encoding another JSON object inside the output string."
    };
    let request = NodeRequestFixture {
        run_id: if next {
            "live-checkpoint-after"
        } else {
            "live-checkpoint-before"
        },
        node: "worker",
        node_instance: 1,
        execution: if next { 2 } else { 1 },
        worker: "agent.worker@1",
        instructions,
        input: Value::Null,
        binding: binding.clone(),
        environment: settings.connection_values(&binding),
    }
    .into_request();
    let session = adapter
        .sessions
        .open(&request.invocation, &request.environment)
        .await
        .assert_value();
    if let Some(restored) = restored {
        assert!(session.restore_conversation(restored).await);
    }
    let invocation = DriverInvocation {
        node: request.invocation,
        environment: request.environment,
        session: session.clone(),
        dispatch: Default::default(),
        host_context: None,
        role: NodeRole::Worker,
        response: NodeResponseContract::Worker {
            output: PayloadType::String,
        },
        provider_session_slot: NodeInstanceId::new(1).assert_value(),
        provider_home_slot: NodeInstanceId::new(if next { 8 } else { 1 }).assert_value(),
        auxiliary: false,
    };
    let (_cancel, control, mut output) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Ok(event) = output.recv().await {
            events.push(event);
        }
        events
    });
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        adapter.driver.run(invocation, control),
    )
    .await
    .assert_value();
    let events = drain.await.assert_value();
    let outcome = outcome.unwrap_or_else(|error| {
        panic!("live conversation turn failed: {error}; events: {events:?}")
    });
    let WorkerOutcome::Verified { output, .. } = outcome else {
        panic!("live conversation turn returned {outcome:?}")
    };
    (session, output)
}

#[tokio::test]
#[ignore = "requires explicit ZEROSHOT_LIVE_* provider credentials/model and a disposable matrix copy"]
async fn checkpoint_conversation_live_contract() {
    let mut settings = LiveSettings::read();
    let directory = TestDirectory::new("live-checkpoint-conversation");
    let first = directory.child("first");
    let second = directory.child("second");
    std::fs::create_dir_all(&first).assert_value();
    std::fs::create_dir_all(&second).assert_value();
    let workspace = first.join("workspace");
    crate::native_v2_capsule::provider_process::copy_workspace_entry(
        &settings.workspace,
        &workspace,
    )
    .assert_value();
    isolated(&mut settings, &first, workspace.clone());
    let word = format!("fact-{}", uuid::Uuid::now_v7().simple());
    std::fs::write(workspace.join("secret-word"), &word).assert_value();
    let adapter = configured_adapter(&settings, first.join("runtime"));
    let (session, _output) = turn(&adapter, &settings, false, None).await;
    assert!(!workspace.join("secret-word").exists());
    let source = session.conversation().await.assert_value();
    let captured = capture(ConversationSources::from([(
        NodeInstanceId::new(1).assert_value(),
        (NodeName::new("worker").assert_value(), Ok(source.clone())),
    )]));
    assert!(captured.notices.is_empty());
    let files = captured.files.assert_value();
    for name in source.files {
        std::fs::remove_file(source.storage.join(name)).assert_value();
    }
    session.close().await;
    let copied = second.join("workspace");
    crate::native_v2_capsule::provider_process::copy_workspace_entry(&workspace, &copied)
        .assert_value();
    isolated(&mut settings, &second, copied.clone());
    fn absent(path: &std::path::Path, word: &str) {
        for entry in std::fs::read_dir(path).assert_value() {
            let path = entry.assert_value().path();
            if path.is_dir() {
                absent(&path, word);
            } else {
                assert!(
                    !std::fs::read(&path)
                        .assert_value()
                        .windows(word.len())
                        .any(|bytes| bytes == word.as_bytes()),
                    "fact present before restoration: {}",
                    path.display()
                );
            }
        }
    }
    absent(&second, &word);
    let restored = files
        .conversation(
            NodeInstanceId::new(1).assert_value(),
            &NodeName::new("worker").assert_value(),
        )
        .assert_value();
    let adapter = configured_adapter(&settings, second.join("runtime"));
    let (session, output) = turn(&adapter, &settings, true, Some(restored)).await;
    assert!(
        output.as_str().is_some_and(|output| output.contains(&word)),
        "the continued provider must recall the first-turn-only fact: {output}"
    );
    session.close().await;
}
