use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_copilot_acknowledges_after_native_rpc_handoff_once() {
    use crate::active_memory::hooks::ActiveMemoryHooks;
    use crate::harnesses::{HarnessBuildContext, HarnessHost, HarnessRegistry};
    use crate::native_v2_candidate::{NativeV2CandidateConfig, build_local_native_v2_candidate};
    use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;
    use crate::v2_run_ledger::{CreateRun, RunLedger};
    use openengine_cluster_protocol::{RunId, RunSteerParams, RunVectorContent, Sha256Digest};
    let directory = TestDirectory::new("copilot-steer");
    let executable = directory.write_executable("copilot", &script());
    let mut values = fixture_values(&directory, "steer");
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
    let _ = complete(handle).await;
    let replies: Value =
        serde_json::from_str(&directory.read("capture.steer-replies")).assert_value();
    assert!(
        replies[0]["result"]["output"]["additionalContext"]
            .as_str()
            .assert_value()
            .contains(&"x".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES))
    );
    assert_eq!(
        crate::v2_run_ledger::steering::all(ledger.as_ref(), &run)
            .await
            .assert_value()
            .iter()
            .filter(|v| matches!(v.content, RunVectorContent::SteerReceipt { .. }))
            .count(),
        1
    );
    runner.close_run(&run).await;
}
