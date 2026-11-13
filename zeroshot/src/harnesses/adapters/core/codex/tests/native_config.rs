use super::*;

#[tokio::test]
async fn native_args_survive_correction_and_revisit_without_leaking() {
    let directory = TestDirectory::new("codex-native-config");
    let capture = directory.child("capture");
    let adapter = scripted_adapter(&directory, CodexProvider::OpenAi);
    let mut binding = binding_with_model(
        "opaque",
        SessionScope::NodeInstance,
        &["CAPTURE_PATH", "CORRECT_OUTPUT", "OPENAI_API_KEY"],
    );
    crate::harnesses::native_config::tests::set(
        &mut binding,
        json!({"args":["--config", "web_search=\"disabled\""]}),
    );
    let admitted = admitted(binding, CodexProvider::OpenAi).await;
    let runtime = runner(&admitted, adapter.clone());
    let values = [
        ("CAPTURE_PATH", capture.display().to_string()),
        ("CORRECT_OUTPUT", "true".to_owned()),
        ("OPENAI_API_KEY", "fixture-key".to_owned()),
    ];
    for execution in [1, 2] {
        let (_, outcome) =
            complete_with_logs(start(&runtime, &admitted, execution, &values).await).await;
        assert!(matches!(
            outcome.assert_value(),
            WorkerOutcome::Verified { .. }
        ));
    }
    let recorded = fs::read_to_string(&capture).assert_value();
    assert_eq!(recorded.matches("arg=web_search=\"disabled\"").count(), 3);
    assert!(!recorded.contains("arg=--dangerously-bypass-approvals-and-sandbox"));
    let clean = admitted_with_default_config(&directory, adapter, &capture).await;
    assert!(!clean.contains("web_search="));
}

#[tokio::test]
async fn native_args_compose_with_foundry_on_correction_and_revisit() {
    let directory = TestDirectory::new("codex-foundry-native-config");
    let capture = directory.child("capture");
    let adapter = scripted_adapter(&directory, CodexProvider::Foundry);
    let mut binding = binding_with_model(
        "provider-owned-deployment",
        SessionScope::NodeInstance,
        &[
            "CAPTURE_PATH",
            "CORRECT_OUTPUT",
            "AZURE_FOUNDRY_API_KEY",
            "AZURE_FOUNDRY_RESOURCE",
        ],
    );
    crate::harnesses::native_config::tests::set(
        &mut binding,
        json!({"args":["--config", "web_search=\"disabled\""]}),
    );
    let admitted = admitted(binding, CodexProvider::Foundry).await;
    let runtime = runner(&admitted, adapter);
    let values = [
        ("CAPTURE_PATH", capture.display().to_string()),
        ("CORRECT_OUTPUT", "true".to_owned()),
        ("AZURE_FOUNDRY_API_KEY", "fixture-key".to_owned()),
        ("AZURE_FOUNDRY_RESOURCE", "contoso-ai".to_owned()),
    ];
    for execution in [1, 2] {
        let (_, outcome) =
            complete_with_logs(start(&runtime, &admitted, execution, &values).await).await;
        outcome.assert_value();
    }
    let recorded = fs::read_to_string(&capture).assert_value();
    for expected in [
        "arg=web_search=\"disabled\"",
        "arg=model_provider=\"foundry\"",
        "arg=model_providers.foundry.env_http_headers={\"api-key\"=\"AZURE_FOUNDRY_API_KEY\"}",
        "arg=model_providers.foundry.requires_openai_auth=false",
        "foundry_key=fixture-key",
    ] {
        assert_eq!(recorded.matches(expected).count(), 3, "{recorded}");
    }
    assert_eq!(recorded.matches("arg=resume\n").count(), 2);
    assert!(!recorded.contains("arg=--dangerously-bypass-approvals-and-sandbox"));
    assert!(!recorded.contains("arg=fixture-key"));
}

async fn admitted_with_default_config(
    directory: &TestDirectory,
    adapter: Arc<NativeV2CodexAdapter>,
    capture: &std::path::Path,
) -> String {
    let plain = admitted(
        binding_with_model(
            "opaque",
            SessionScope::Execution,
            &["CAPTURE_PATH", "OPENAI_API_KEY"],
        ),
        CodexProvider::OpenAi,
    )
    .await;
    let runtime = runner(&plain, adapter);
    let path = directory.child("plain-capture");
    let (_, outcome) = complete_with_logs(
        start(
            &runtime,
            &plain,
            3,
            &[
                ("CAPTURE_PATH", path.display().to_string()),
                ("OPENAI_API_KEY", "fixture-key".to_owned()),
            ],
        )
        .await,
    )
    .await;
    outcome.assert_value();
    assert!(capture.exists());
    fs::read_to_string(path).assert_value()
}

#[tokio::test]
async fn native_startup_rejection_is_terminal_and_preserves_stderr() {
    let directory = TestDirectory::new("codex-native-config-rejection");
    let source = "#!/bin/sh\nprintf 'launch\\n' >> launches\nprintf '%s\\n' 'error: invalid value for --sandbox: native-value' >&2\nexit 2\n";
    let adapter = scripted_adapter_with(&directory, CodexProvider::OpenAi, "codex", source);
    let mut binding = binding_with_model("opaque", SessionScope::Execution, &["OPENAI_API_KEY"]);
    crate::harnesses::native_config::tests::set(
        &mut binding,
        json!({"args":["--sandbox","native-value"]}),
    );
    let admitted = admitted(binding, CodexProvider::OpenAi).await;
    let runtime = runner(&admitted, adapter);
    let (logs, result) = complete_with_logs(
        start(
            &runtime,
            &admitted,
            1,
            &[("OPENAI_API_KEY", "fixture-key".to_owned())],
        )
        .await,
    )
    .await;
    let error = result.unwrap_err().to_string();
    assert!(
        error.contains("--sandbox") && error.contains("codex") && error.contains("node `work`"),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(directory.child("workspace/launches"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(!logs.contains("requesting correction"));
}

#[test]
fn contained_native_configuration_cannot_override_host_policy() {
    for args in [
        vec!["--sandbox", "workspace-write"],
        vec!["-c", "approval_policy=never"],
        vec!["-c=sandbox_mode=read-only"],
    ] {
        let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
        assert!(super::super::native_config::validate_contained(&args).is_err());
    }
}

#[tokio::test]
async fn native_exec_subcommands_are_rejected_before_process_launch() {
    let admitted = admitted(
        binding_with_model("opaque", SessionScope::NodeInstance, &["OPENAI_API_KEY"]),
        CodexProvider::OpenAi,
    )
    .await;
    for command in ["resume", "fork", "review", "help"] {
        let directory = TestDirectory::new("codex-native-subcommand-rejection");
        let adapter = scripted_adapter_with(
            &directory,
            CodexProvider::OpenAi,
            "codex",
            "#!/bin/sh\nprintf 'launch\\n' >> launches\nexit 2\n",
        );
        let mut request = request(
            &admitted,
            1,
            &[("OPENAI_API_KEY", "fixture-key".to_owned())],
        );
        crate::harnesses::native_config::tests::set(
            &mut request.invocation.binding,
            json!({"args":["--color","never",command,"foreign-thread"]}),
        );
        // Exercise the adapter boundary directly, independently of registry admission.
        let (result, logs, session) =
            crate::native_v2_runner::test_support::run_null_worker(adapter.as_ref(), request).await;
        let detail = result.unwrap_err().to_string();
        for expected in [
            "node `work`",
            "harness `codex`",
            "harnessConfig.args",
            command,
        ] {
            assert!(detail.contains(expected), "{detail}");
        }
        assert!(!directory.child("workspace/launches").exists());
        assert!(!logs.contains("continuing") && !logs.contains("requesting correction"));
        session.close().await;
        assert!(!session.is_live().await);
    }
}
