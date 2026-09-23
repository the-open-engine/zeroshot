use super::*;
use crate::native_v2_capsule::provider_process::{
    LocalHarnessEnvironment, redaction_values, safe_provider_text,
};

fn local_configuration(
    provider: CodexProvider,
    native_environment: BTreeMap<String, String>,
) -> NativeV2CodexConfig {
    NativeV2CodexConfig {
        provider,
        executable: PathBuf::from("codex"),
        workspace: PathBuf::from("/workspace"),
        runtime_home: PathBuf::from("/runtime"),
        local_user: Some(NativeV2CodexUser {
            home: PathBuf::from("/user"),
            codex_home: PathBuf::from("/user/.codex"),
        }),
        native_environment: LocalHarnessEnvironment::new(native_environment),
        search_path: "/usr/bin:/bin".to_owned(),
        process_pool: HostedProcessPool::new(10_002, 10_002, 20_000, 20_000).assert_value(),
    }
}

#[test]
fn coverage_contract_codex_local_configuration_filters_cross_provider_auth_but_keeps_transport() {
    let native = BTreeMap::from([
        ("CODEX_API_KEY".to_owned(), "codex-secret".to_owned()),
        ("OPENAI_API_KEY".to_owned(), "openai-secret".to_owned()),
        (
            "OPENAI_BASE_URL".to_owned(),
            "https://native.invalid".to_owned(),
        ),
        ("HTTPS_PROXY".to_owned(), "https://proxy.invalid".to_owned()),
    ]);
    let openai =
        NativeV2CodexAdapter::new_local(local_configuration(CodexProvider::OpenAi, native.clone()));
    assert!(openai.local_environment.contains_key("CODEX_API_KEY"));
    assert!(openai.local_environment.contains_key("OPENAI_API_KEY"));

    let openrouter =
        NativeV2CodexAdapter::new_local(local_configuration(CodexProvider::OpenRouter, native));
    assert!(!openrouter.local_environment.contains_key("CODEX_API_KEY"));
    assert!(!openrouter.local_environment.contains_key("OPENAI_API_KEY"));
    assert!(openrouter.local_environment.contains_key("OPENAI_BASE_URL"));
    assert!(openrouter.local_environment.contains_key("HTTPS_PROXY"));
}

#[test]
fn coverage_contract_codex_declared_auth_suppresses_ambient_auth_and_paths_fail_closed() {
    let declared = binding(SessionScope::Execution, &["OPENAI_API_KEY"]);
    let resolved = ResolvedEnvironment::exact(
        &declared,
        BTreeMap::from([(environment_name("OPENAI_API_KEY"), "declared".to_owned())]),
    )
    .assert_value();
    let mut values = BTreeMap::new();
    merge_local_environment(
        &mut values,
        &resolved,
        &BTreeMap::from([
            ("OPENAI_API_KEY".to_owned(), "ambient".to_owned()),
            (
                "OPENAI_BASE_URL".to_owned(),
                "https://native.invalid".to_owned(),
            ),
        ]),
    );
    assert!(!values.contains_key("OPENAI_API_KEY"));
    assert!(values.contains_key("OPENAI_BASE_URL"));

    let empty_binding = binding(SessionScope::Execution, &[]);
    let empty = ResolvedEnvironment::exact(&empty_binding, BTreeMap::new()).assert_value();
    let adapter =
        NativeV2CodexAdapter::new(local_configuration(CodexProvider::OpenAi, BTreeMap::new()));
    let error = adapter
        .provider_environment(&empty, Path::new(""))
        .err()
        .assert_value();
    assert!(error.to_string().contains("runtime home"));

    #[cfg(unix)]
    {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let mut configuration = local_configuration(CodexProvider::OpenAi, BTreeMap::new());
        configuration.local_user = Some(NativeV2CodexUser {
            home: PathBuf::from(OsString::from_vec(vec![0xff])),
            codex_home: PathBuf::from("/user/.codex"),
        });
        let error = NativeV2CodexAdapter::new_local(configuration)
            .provider_environment(&empty, Path::new("/runtime"))
            .err()
            .assert_value();
        assert!(error.to_string().contains("user home"));
    }
}

#[test]
fn command_is_exact_and_rejects_adapter_owned_collisions() {
    let declared = binding(SessionScope::Execution, &["DECLARED"]);
    let resolved = ResolvedEnvironment::exact(
        &declared,
        BTreeMap::from([(
            EnvironmentVariableName::new("DECLARED").assert_value(),
            "resolved-value".to_owned(),
        )]),
    )
    .assert_value();
    let environment = process_environment(
        &resolved,
        "/private/runtime".to_owned(),
        "/private/runtime".to_owned(),
        "/usr/bin:/bin".to_owned(),
    )
    .assert_value();
    assert_eq!(
        environment,
        BTreeMap::from([
            ("CODEX_HOME".to_owned(), "/private/runtime".to_owned()),
            ("DECLARED".to_owned(), "resolved-value".to_owned()),
            ("HOME".to_owned(), "/private/runtime".to_owned()),
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ])
    );

    let binding = binding(SessionScope::Execution, &["CODEX_HOME"]);
    let environment = ResolvedEnvironment::exact(
        &binding,
        BTreeMap::from([(
            EnvironmentVariableName::new("CODEX_HOME").assert_value(),
            "node-owned".to_owned(),
        )]),
    )
    .assert_value();
    assert_eq!(
        process_environment(
            &environment,
            "adapter-owned".to_owned(),
            "adapter-owned".to_owned(),
            "/usr/bin:/bin".to_owned()
        ),
        Err(NodeRunnerError::Driver)
    );
}

pub(super) fn provider_environment(
    provider: CodexProvider,
    values: &[(&str, &str)],
) -> Result<BTreeMap<String, String>, NodeRunnerError> {
    let directory = TestDirectory::new("codex-bedrock-environment");
    let declared = values.iter().map(|(name, _)| *name).collect::<Vec<_>>();
    let binding = binding(SessionScope::Execution, declared.as_slice());
    let resolved_values = BTreeMap::from_iter(
        values
            .iter()
            .copied()
            .map(|(name, value)| (environment_name(name), value.to_owned())),
    );
    let resolved = ResolvedEnvironment::exact(&binding, resolved_values).assert_value();
    NativeV2CodexAdapter::new(scripted_adapter(&directory, provider).config.clone())
        .provider_environment(&resolved, &directory.child("runtime-home"))
}

#[test]
fn bedrock_environment_requires_aws_values_and_rejects_conflicting_codex_controls() {
    let required = [
        (AWS_BEARER_TOKEN_BEDROCK, "bedrock-secret"),
        (AWS_REGION, "us-east-1"),
    ];
    let valid = provider_environment(CodexProvider::Bedrock, &required).assert_value();
    for (name, expected) in required {
        assert_eq!(valid.get(name).map(String::as_str), Some(expected));
    }

    for omitted_or_empty in [AWS_BEARER_TOKEN_BEDROCK, AWS_REGION] {
        let omitted = required
            .into_iter()
            .filter(|(name, _)| *name != omitted_or_empty)
            .collect::<Vec<_>>();
        assert!(
            provider_environment(CodexProvider::Bedrock, &omitted).is_err(),
            "accepted missing {omitted_or_empty}"
        );
        let empty = required.map(|(name, value)| {
            if name == omitted_or_empty {
                (name, "")
            } else {
                (name, value)
            }
        });
        assert!(
            provider_environment(CodexProvider::Bedrock, &empty).is_err(),
            "accepted empty {omitted_or_empty}"
        );
    }

    for conflict in ["CODEX_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY"] {
        let values = [
            (AWS_BEARER_TOKEN_BEDROCK, "bedrock-secret"),
            (AWS_REGION, "us-east-1"),
            (conflict, "conflict"),
        ];
        assert!(
            provider_environment(CodexProvider::Bedrock, &values).is_err(),
            "accepted conflicting {conflict}"
        );
    }
}

fn assert_bedrock_capture(capture: &str) {
    for expected in [
        "arg=model_provider=\"amazon-bedrock\"",
        "arg=--model\narg=global.anthropic.provider-owned-model",
        "bedrock_key=fake-bedrock-key",
        "aws_region=us-east-1",
        "openai_key=unset",
        "openrouter_key=unset",
        "codex_key=unset",
    ] {
        assert!(
            capture.contains(expected),
            "missing capture evidence: {expected}"
        );
    }
    for forbidden in [
        "model_providers.amazon-bedrock",
        "bedrock-runtime.",
        "base_url=",
    ] {
        assert!(
            !capture.contains(forbidden),
            "unexpected custom Bedrock configuration: {forbidden}"
        );
    }
    assert_schema_capture(capture);
}

#[tokio::test]
async fn bedrock_script_uses_the_builtin_provider_and_preserves_aws_environment() {
    let directory = TestDirectory::new("codex-bedrock");
    let capture = directory.child("capture");
    let adapter = scripted_adapter(&directory, CodexProvider::Bedrock);
    let admitted = admitted(
        binding_with_model(
            "global.anthropic.provider-owned-model",
            SessionScope::Execution,
            &["CAPTURE_PATH", AWS_BEARER_TOKEN_BEDROCK, AWS_REGION],
        ),
        CodexProvider::Bedrock,
    )
    .await;
    let runtime = runner(&admitted, adapter);
    let handle = start(
        &runtime,
        &admitted,
        1,
        &[
            ("CAPTURE_PATH", capture.display().to_string()),
            (AWS_BEARER_TOKEN_BEDROCK, "fake-bedrock-key".to_owned()),
            (AWS_REGION, "us-east-1".to_owned()),
        ],
    )
    .await;
    complete_verified_with_logs(handle).await;

    assert_bedrock_capture(&fs::read_to_string(capture).assert_value());
}

#[test]
fn log_redactions_are_longest_first_and_do_not_leave_overlapping_suffixes() {
    let binding = binding(SessionScope::Execution, &["LONG_SECRET", "SHORT_SECRET"]);
    let environment = ResolvedEnvironment::exact(
        &binding,
        BTreeMap::from([
            (
                EnvironmentVariableName::new("LONG_SECRET").assert_value(),
                "secret-tail".to_owned(),
            ),
            (
                EnvironmentVariableName::new("SHORT_SECRET").assert_value(),
                "secret".to_owned(),
            ),
        ]),
    )
    .assert_value();
    let redactions = redaction_values(environment.iter().map(|(_, value)| value));
    assert_eq!(redactions, vec!["secret-tail", "secret"]);
    assert_eq!(
        safe_provider_text("value=secret-tail\0after", &redactions),
        "value=[REDACTED]\u{fffd}after"
    );
}

#[test]
fn gateway_environment_rejects_conflicting_provider_credentials() {
    let required = [
        ("GATEWAY_BASE_URL", "https://gateway.example/api"),
        ("GATEWAY_API_KEY", "gateway-secret"),
    ];
    assert!(provider_environment(CodexProvider::Gateway, &required).is_ok());
    for conflict in [
        "CODEX_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "AWS_BEARER_TOKEN_BEDROCK",
    ] {
        let mut values = required.to_vec();
        values.push((conflict, "conflict"));
        assert!(
            provider_environment(CodexProvider::Gateway, &values).is_err(),
            "accepted {conflict}"
        );
    }
}

#[test]
fn hosted_codex_preserves_declared_endpoint_overrides_for_all_providers() {
    for (provider, required) in [
        (
            CodexProvider::OpenAi,
            vec![("OPENAI_API_KEY", "openai-secret")],
        ),
        (
            CodexProvider::OpenRouter,
            vec![("OPENROUTER_API_KEY", "router-secret")],
        ),
        (
            CodexProvider::Bedrock,
            vec![
                (AWS_BEARER_TOKEN_BEDROCK, "bedrock-secret"),
                (AWS_REGION, "us-east-1"),
            ],
        ),
        (
            CodexProvider::Gateway,
            vec![
                ("GATEWAY_BASE_URL", "https://gateway.example/api"),
                ("GATEWAY_API_KEY", "gateway-secret"),
            ],
        ),
    ] {
        for name in crate::native_v2_capsule::provider_process::CODEX_LOCAL_ENVIRONMENT
            .iter()
            .copied()
            .filter(|name| !matches!(*name, "CODEX_API_KEY" | "OPENAI_API_KEY"))
        {
            let mut values = required.clone();
            values.push((name, "https://proxy.example/custom"));
            let environment = provider_environment(provider, &values).assert_value();
            assert_eq!(
                environment.get(name).map(String::as_str),
                Some("https://proxy.example/custom")
            );
        }
    }
}

#[test]
fn codex_declared_connection_overrides_local_endpoint_and_redacts_both() {
    let directory = TestDirectory::new("codex-endpoint-precedence");
    let mut adapter = NativeV2CodexAdapter::new_for_test(
        scripted_adapter(&directory, CodexProvider::OpenAi)
            .config
            .clone(),
    );
    adapter.local_environment.insert(
        "OPENAI_BASE_URL".to_owned(),
        "https://shell.example/private".to_owned(),
    );
    let binding = binding(
        SessionScope::Execution,
        &["OPENAI_API_KEY", "OPENAI_BASE_URL"],
    );
    let resolved = ResolvedEnvironment::exact(
        &binding,
        BTreeMap::from([
            (
                environment_name("OPENAI_API_KEY"),
                "declared-key".to_owned(),
            ),
            (
                environment_name("OPENAI_BASE_URL"),
                "https://connection.example/private".to_owned(),
            ),
        ]),
    )
    .assert_value();
    let values = adapter
        .provider_environment(&resolved, &directory.child("runtime"))
        .assert_value();
    assert_eq!(
        values["OPENAI_BASE_URL"],
        "https://connection.example/private"
    );
    adapter
        .local_environment
        .insert("CLAUDE_CODE_USE_GATEWAY".to_owned(), "1".to_owned());
    let redactions = provider_redactions(&resolved, &adapter.local_environment);
    assert_eq!(
        safe_provider_text(
            "HTTP 401, retry 1: https://shell.example/private",
            &redactions
        ),
        "HTTP 401, retry 1: [REDACTED]"
    );
    assert!(redactions.contains(&"https://shell.example/private".to_owned()));
    assert!(redactions.contains(&"https://connection.example/private".to_owned()));
    assert!(
        NativeV2CodexAdapter::new(adapter.config.clone())
            .local_environment
            .is_empty()
    );
}
