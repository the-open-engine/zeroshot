use super::*;

async fn corrected_output(
    directory: &TestDirectory,
    provider: CodexProvider,
) -> (WorkerOutcome, String, String) {
    let capture = directory.child("capture");
    let configuration = match provider {
        CodexProvider::OpenAi => Some(("OPENAI_API_KEY", "fake-openai-key", "gpt-5.6-sol")),
        CodexProvider::OpenRouter => Some((
            "OPENROUTER_API_KEY",
            "fake-openrouter-key",
            "openai/gpt-5.6-sol",
        )),
        CodexProvider::Gateway => Some(("GATEWAY_API_KEY", "fake-gateway-key", "opaque/model")),
        CodexProvider::Bedrock | CodexProvider::Foundry => None,
    }
    .assert_value_with("correction fixture supports OpenAI, OpenRouter, and gateway");
    let (credential_name, credential_value, model) = configuration;
    let mut names = vec!["CAPTURE_PATH", "CORRECT_OUTPUT", credential_name];
    let mut values = vec![
        ("CAPTURE_PATH", capture.display().to_string()),
        ("CORRECT_OUTPUT", "true".to_owned()),
        (credential_name, credential_value.to_owned()),
    ];
    if provider == CodexProvider::Gateway {
        names.push("GATEWAY_BASE_URL");
        values.push((
            "GATEWAY_BASE_URL",
            "https://gateway.example/quoted\"path/v1".to_owned(),
        ));
    }
    let adapter = scripted_adapter(directory, provider);
    let admitted = admitted(
        binding_with_model(model, SessionScope::Execution, &names),
        provider,
    )
    .await;
    let runtime = runner(&admitted, adapter);
    let handle = start(&runtime, &admitted, 1, &values).await;
    let (logs, outcome) = complete_with_logs(handle).await;
    (
        outcome.assert_value(),
        fs::read_to_string(capture).assert_value(),
        logs,
    )
}

#[tokio::test]
async fn invalid_output_is_corrected_in_the_same_codex_session() {
    let directory = TestDirectory::new("codex-correction");
    let (outcome, capture, logs) = corrected_output(&directory, CodexProvider::OpenAi).await;

    assert!(matches!(
        outcome,
        WorkerOutcome::Verified { output, .. } if output == json!({"answer": 43})
    ));
    assert_eq!(capture.matches("arg=resume").count(), 1);
    assert_eq!(capture.matches("arg=thread-123").count(), 1);
    assert!(
        capture.contains("Your previous final response was rejected mechanically"),
        "missing correction prompt; fixture capture:\n{capture}\nprovider logs:\n{logs}"
    );
    assert!(capture.contains("output $.answer must be a integer"));
    assert!(logs.contains("Codex final output rejected; requesting correction"));
    assert_eq!(
        logs.matches("final output rejected: output $.answer must be a integer")
            .count(),
        1
    );
    assert!(!logs.contains("Your previous final response was rejected mechanically"));
}

#[tokio::test]
async fn openrouter_correction_scopes_configuration_to_the_resume_command() {
    let directory = TestDirectory::new("codex-openrouter-correction");
    let (outcome, capture, _logs) = corrected_output(&directory, CodexProvider::OpenRouter).await;

    assert!(matches!(
        outcome,
        WorkerOutcome::Verified { output, .. } if output == json!({"answer": 43})
    ));
    let resumed = capture.rsplit_once("---\n").assert_value().1;
    let resume_index = resumed.find("arg=resume\n").assert_value();
    let sandbox_index = resumed
        .find("arg=--dangerously-bypass-approvals-and-sandbox\n")
        .assert_value();
    assert!(sandbox_index < resume_index);
    for expected in [
        "arg=model_provider=\"openrouter\"\n",
        "arg=model_providers.openrouter.base_url=\"https://openrouter.ai/api/v1\"\n",
        "arg=model_providers.openrouter.env_key=\"OPENROUTER_API_KEY\"\n",
        "arg=model_providers.openrouter.wire_api=\"responses\"\n",
        "arg=model_reasoning_effort=\"max\"\n",
    ] {
        assert!(
            resumed.find(expected).assert_value() > resume_index,
            "resume-scoped argument appeared before resume: {expected}"
        );
    }
    assert!(resumed.contains("arg=--model\narg=openai/gpt-5.6-sol\n"));
    assert!(resumed.contains("arg=thread-123\narg=-\n"));
    assert!(!capture.contains("arg=web_search="));
    assert!(!capture.contains("arg=sandbox_workspace_write.network_access="));
}

#[tokio::test]
async fn malformed_output_stops_after_two_correction_turns() {
    let directory = TestDirectory::new("codex-malformed-limit");
    let capture = directory.child("capture");
    let (admitted, runtime) = openai_runtime(
        &directory,
        SessionScope::Execution,
        &["ALWAYS_MALFORMED", "CAPTURE_PATH", "OPENAI_API_KEY"],
    )
    .await;
    let handle = start(
        &runtime,
        &admitted,
        1,
        &[
            ("ALWAYS_MALFORMED", "true".to_owned()),
            ("CAPTURE_PATH", capture.display().to_string()),
            ("OPENAI_API_KEY", "fake-openai-key".to_owned()),
        ],
    )
    .await;

    let (logs, outcome) = complete_with_logs(handle).await;
    assert_eq!(outcome.assert_value(), WorkerOutcome::malformed());
    assert_eq!(
        logs.matches("final output rejected: output $.answer must be a integer")
            .count(),
        3
    );
    let capture = fs::read_to_string(capture).assert_value();
    assert_eq!(capture.matches("prompt=").count(), 3);
    assert_eq!(capture.matches("arg=resume").count(), 2);
}

#[tokio::test]
async fn gateway_correction_keeps_endpoint_auth_and_model_on_resume() {
    let directory = TestDirectory::new("codex-gateway-correction");
    let (outcome, capture, logs) = corrected_output(&directory, CodexProvider::Gateway).await;
    assert!(
        matches!(outcome, WorkerOutcome::Verified { output, .. } if output == json!({"answer":43}))
    );
    assert_eq!(capture.matches("arg=resume\n").count(), 1);
    let expected_url =
        serde_json::to_string("https://gateway.example/quoted\"path/v1").assert_value();
    for turn in capture.split("---\n").skip(1) {
        for expected in [
            "arg=model_provider=\"gateway\"".to_owned(),
            format!("arg=model_providers.gateway.base_url={expected_url}"),
            "arg=model_providers.gateway.env_key=\"GATEWAY_API_KEY\"".to_owned(),
            "arg=model_providers.gateway.wire_api=\"responses\"".to_owned(),
            "arg=model_providers.gateway.requires_openai_auth=false".to_owned(),
            "arg=--model\narg=opaque/model".to_owned(),
            "gateway_key=fake-gateway-key".to_owned(),
        ] {
            assert!(turn.contains(&expected), "missing {expected}");
        }
        assert!(
            !turn
                .lines()
                .any(|line| line.starts_with("arg=") && line.contains("fake-gateway-key"))
        );
    }
    assert!(logs.contains("visible [REDACTED]"));
    assert!(!logs.contains("fake-gateway-key"));
}

#[tokio::test]
async fn dispatch_metadata_survives_corrections_and_session_revisits() {
    use crate::native_v2_candidate::test_support::{
        CAPTURE_EXECUTION_ENVIRONMENT, assert_execution_environment,
    };
    use crate::native_v2_runner::NodeDispatchMetadata;
    let directory = TestDirectory::new("codex-dispatch-metadata");
    let source = SCRIPT.replace(
        "set -eu",
        &format!("set -eu\n{CAPTURE_EXECUTION_ENVIRONMENT}"),
    );
    let adapter =
        scripted_adapter_with(&directory, CodexProvider::OpenAi, "codex-context", &source);
    let admitted = admitted(
        binding(
            SessionScope::NodeInstance,
            &[
                "CAPTURE_PATH",
                "CORRECT_OUTPUT",
                "OPENAI_API_KEY",
                "ZEROSHOT_NODE_NAME",
            ],
        ),
        CodexProvider::OpenAi,
    )
    .await;
    let runtime = runner(&admitted, adapter);
    let values = [
        (
            "CAPTURE_PATH",
            directory.child("capture").display().to_string(),
        ),
        ("CORRECT_OUTPUT", "true".to_owned()),
        ("OPENAI_API_KEY", "fake-key".to_owned()),
        ("ZEROSHOT_NODE_NAME", "spoofed".to_owned()),
    ];
    let mut first = request(&admitted, 1, &values);
    first.dispatch = NodeDispatchMetadata {
        attempt: 2,
        iteration: 3,
    };
    let mut second = request(&admitted, 2, &values);
    second.dispatch = NodeDispatchMetadata {
        attempt: 1,
        iteration: 4,
    };
    for visit in [&first, &second] {
        let (_, result) =
            complete_with_logs(runtime.start(visit.clone()).await.assert_value()).await;
        result.assert_value();
    }
    let capture = fs::read_to_string(directory.child("workspace/execution-context")).assert_value();
    let entries = capture.lines().collect::<Vec<_>>();
    assert_eq!(entries.len(), 21); // First visit plus correction, then a reused session.
    for (chunk, visit) in entries.chunks_exact(7).zip([&first, &first, &second]) {
        assert_execution_environment(
            &(chunk.join("\n") + "\n"),
            &visit.invocation.reference,
            visit.dispatch,
            "step",
        );
    }
}

#[tokio::test]
async fn dispatch_metadata_is_isolated_between_concurrent_node_instances() {
    use crate::native_v2_candidate::test_support::{
        CAPTURE_EXECUTION_ENVIRONMENT, assert_execution_environment,
    };
    use crate::native_v2_contract::NodeInstanceId;
    use crate::native_v2_runner::NodeDispatchMetadata;
    let directory = TestDirectory::new("codex-concurrent-metadata");
    let capture = CAPTURE_EXECUTION_ENVIRONMENT.replace(
        " >> execution-context",
        " >> execution-context-$ZEROSHOT_NODE_INSTANCE_ID",
    );
    let source = SCRIPT.replace("set -eu", &format!("set -eu\n{capture}"));
    let adapter =
        scripted_adapter_with(&directory, CodexProvider::OpenAi, "codex-context", &source);
    let admitted = admitted(
        binding(
            SessionScope::NodeInstance,
            &["CAPTURE_PATH", "OPENAI_API_KEY"],
        ),
        CodexProvider::OpenAi,
    )
    .await;
    let runtime = runner(&admitted, adapter);
    let first_values = [
        (
            "CAPTURE_PATH",
            directory.child("capture-1").display().to_string(),
        ),
        ("OPENAI_API_KEY", "fake-key".to_owned()),
    ];
    let second_values = [
        (
            "CAPTURE_PATH",
            directory.child("capture-2").display().to_string(),
        ),
        ("OPENAI_API_KEY", "fake-key".to_owned()),
    ];
    let mut first = request(&admitted, 1, &first_values);
    first.dispatch = NodeDispatchMetadata {
        attempt: 2,
        iteration: 3,
    };
    let mut second = request(&admitted, 2, &second_values);
    second.invocation.reference.node_instance = NodeInstanceId::new(2).assert_value();
    second.dispatch = NodeDispatchMetadata {
        attempt: 1,
        iteration: 4,
    };
    let (left, right) = tokio::join!(runtime.start(first.clone()), runtime.start(second.clone()));
    let (left, right) = tokio::join!(
        complete_with_logs(left.assert_value()),
        complete_with_logs(right.assert_value())
    );
    left.1.assert_value();
    right.1.assert_value();
    for visit in [first, second] {
        let capture = fs::read_to_string(directory.child(&format!(
            "workspace/execution-context-{}",
            visit.invocation.reference.node_instance
        )))
        .assert_value();
        assert_execution_environment(
            &capture,
            &visit.invocation.reference,
            visit.dispatch,
            "step",
        );
    }
}
