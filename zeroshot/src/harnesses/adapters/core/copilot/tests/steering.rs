use super::*;

struct SteerContext(String);

impl crate::native_v2_runner::AgentContextProvider for SteerContext {
    fn context(&self) -> String {
        self.0.clone()
    }
}

#[tokio::test]
async fn steer_copilot_streams_aggregate_prompts_across_the_inbound_rpc_bound() {
    for count in [255, 256] {
        let mut context = String::from("<steer>\n");
        for i in 0..count {
            context.push_str(&format!(
                "[steer-{i}] {i:03}{}\n",
                "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES - 3)
            ));
        }
        context.push_str("</steer>\nUTF-8: 🦀\t\"\\\r\n");
        let fixture = Fixture::new("success").await;
        let request = fixture.request(1);
        let runtime = fixture
            .runtime
            .with_agent_context(Some(Arc::new(SteerContext(context.clone()))));
        verified(
            complete(runtime.start(request).await.assert_value())
                .await
                .1,
        );
        let capture = fixture.directory.read("capture");
        let send: Value = capture
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).assert_value())
            .find(|message| message["method"] == "session.send")
            .assert_value();
        let prompt = send["params"]["prompt"].as_str().assert_value();
        assert_eq!(prompt.len() > super::super::framing::MAX_BODY, count == 256);
        assert!(prompt.starts_with(&format!("Runtime-owned execution context:\n{context}\n\n")));
        assert_eq!(prompt.matches(INSTRUCTIONS).count(), 1);
        assert_eq!(prompt.matches("[steer-").count(), count);
    }
}

// The steer goes into the running turn as a user message of its own, never as hook context, and
// is received once Copilot shows it in the conversation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_copilot_sends_the_steer_into_the_running_turn_and_receipts_it_once_shown() {
    let (steer, receipts) = steered_turn("steer").await;
    let prompt = steer["params"]["prompt"].as_str().assert_value();
    assert!(prompt.starts_with("<steer>\n") && prompt.ends_with("</steer>"));
    assert!(prompt.contains(&"x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES)));
    assert_eq!(receipts, 1);
}

// A CLI that refuses the message never shows it: the turn goes on and the steer stays due.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_copilot_refused_by_the_cli_is_not_receipted_and_does_not_fail_the_turn() {
    let (_, receipts) = steered_turn("steer_refused").await;
    assert_eq!(receipts, 0);
}

/// One turn of the fake CLI that calls a tool twice after a steer was recorded. Returns the
/// request that carried the steer and the number of receipts the run recorded.
async fn steered_turn(mode: &str) -> (Value, usize) {
    use crate::active_memory::hooks::ActiveMemoryHooks;
    use crate::harnesses::{HarnessBuildContext, HarnessHost, HarnessRegistry};
    use crate::native_v2_candidate::{NativeV2CandidateConfig, build_local_native_v2_candidate};
    use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;
    use crate::v2_run_ledger::{CreateRun, RunLedger};
    use openengine_cluster_protocol::{RunId, RunSteerParams, RunVectorContent, Sha256Digest};
    let directory = TestDirectory::new("copilot-steer");
    let executable = directory.write_executable("copilot", &script());
    let mut values = fixture_values(&directory, mode);
    values.insert(auth::TOKEN.to_owned(), "gho_fake-secret".into());
    let binding = binding(values.keys().cloned());
    let admitted = admitted(binding.clone(), INSTRUCTIONS).await;
    let run = RunId::new("run-copilot");
    let ledger: Arc<dyn RunLedger> = Arc::new(crate::v2_run_ledger::fake::FakeRunLedger::new());
    ledger
        .create_or_get(CreateRun {
            run_id: run.clone(),
            submission_key: IdempotencyKey::new("steer-copilot").assert_value(),
            submission_digest: Sha256Digest::new("a".repeat(64)).assert_value(),
            admitted: admitted.clone(),
        })
        .await
        .assert_value();
    let hooks = Arc::new(ActiveMemoryHooks::empty().assert_value());
    hooks.bind_ledger(run.clone(), ledger.clone());
    let workspace = directory.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    crate::execution::platform::private_directory(&directory.child("runtime")).assert_value();
    let lane = HarnessRegistry::local()
        .configure(HarnessBuildContext {
            lane: admitted.runtime.lane().clone(),
            workspace: workspace.clone(),
            runtime_home: directory.child("runtime"),
            executable,
            prefix_arguments: vec![],
            search_path: "/usr/bin:/bin".into(),
            base_environment: BTreeMap::new(),
            host: HarnessHost::Local {
                home: None,
                environment: LocalHarnessEnvironment::new(BTreeMap::new()),
            },
        })
        .assert_value();
    let runner = build_local_native_v2_candidate(
        &admitted,
        NativeV2CandidateConfig {
            active_memory: Some(hooks),
            lanes: vec![lane],
            workspace,
            delivery: None,
            comment_delivery: None,
        },
    )
    .assert_value();
    let request = NodeRequestFixture {
        run_id: run.as_str(),
        node: "work",
        node_instance: 1,
        execution: 1,
        worker: "agent.work@1",
        instructions: INSTRUCTIONS,
        input: Value::Null,
        binding,
        environment: values
            .into_iter()
            .map(|(k, v)| (environment_name(&k), v))
            .collect(),
    }
    .into_request();
    let handle = runner.start(request).await.assert_value();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !directory.child("capture.steer-held").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .assert_value();
    crate::v2_run_ledger::steering::record(
        ledger.as_ref(),
        RunSteerParams {
            run_id: run.clone(),
            text: "x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES),
        },
    )
    .await
    .assert_value();
    std::fs::write(directory.child("capture.steer-release"), "").assert_value();
    let (_, outcome) = complete(handle).await;
    verified(outcome);
    let replies: Value =
        serde_json::from_str(&directory.read("capture.steer-replies")).assert_value();
    let receipts = crate::v2_run_ledger::steering::all(ledger.as_ref(), &run)
        .await
        .assert_value()
        .iter()
        .filter(|v| matches!(v.content, RunVectorContent::SteerReceipt { .. }))
        .count();
    runner.close_run(&run).await;
    (replies[0].clone(), receipts)
}
