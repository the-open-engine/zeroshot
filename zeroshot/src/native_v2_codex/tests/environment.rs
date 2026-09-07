use super::*;
use crate::native_v2_capsule::provider_process::safe_provider_text;

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

fn bedrock_environment(
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
    scripted_adapter(&directory, CodexProvider::Bedrock)
        .provider_environment(&resolved, &directory.child("runtime-home"))
}

#[test]
fn bedrock_environment_requires_aws_values_and_rejects_conflicting_codex_controls() {
    let required = [
        (AWS_BEARER_TOKEN_BEDROCK, "bedrock-secret"),
        (AWS_REGION, "us-east-1"),
    ];
    let valid = bedrock_environment(&required).assert_value();
    for (name, expected) in required {
        assert_eq!(valid.get(name).map(String::as_str), Some(expected));
    }

    for omitted_or_empty in [AWS_BEARER_TOKEN_BEDROCK, AWS_REGION] {
        let omitted = required
            .into_iter()
            .filter(|(name, _)| *name != omitted_or_empty)
            .collect::<Vec<_>>();
        assert!(
            bedrock_environment(&omitted).is_err(),
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
            bedrock_environment(&empty).is_err(),
            "accepted empty {omitted_or_empty}"
        );
    }

    for conflict in [
        "CODEX_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "CODEX_BASE_URL",
        "OPENAI_BASE_URL",
        "OPENAI_API_BASE",
    ] {
        let values = [
            (AWS_BEARER_TOKEN_BEDROCK, "bedrock-secret"),
            (AWS_REGION, "us-east-1"),
            (conflict, "conflict"),
        ];
        assert!(
            bedrock_environment(&values).is_err(),
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
