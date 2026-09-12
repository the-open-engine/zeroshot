use std::collections::BTreeMap;
use std::path::Path;

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use crate::native_v2_runner::ResolvedEnvironment;

#[test]
fn base_environment_is_explicit_non_secret_and_allows_large_values() {
    assert!(ClaudeProcessEnvironment::new(BTreeMap::new()).is_ok());
    assert!(
        ClaudeProcessEnvironment::new(BTreeMap::from([(
            "OPENAI_API_KEY".to_owned(),
            "not-allowed".to_owned(),
        )]))
        .is_err()
    );
    let long_locale = "x".repeat(20 * 1024);
    let environment =
        ClaudeProcessEnvironment::new(BTreeMap::from([("LANG".to_owned(), long_locale.clone())]))
            .assert_value();
    assert_eq!(
        environment.clone_values().get("LANG").map(String::as_str),
        Some(long_locale.as_str())
    );
    assert!(
        ClaudeProcessEnvironment::new(BTreeMap::from([(
            "LANG".to_owned(),
            "invalid\0value".to_owned(),
        )]))
        .is_err()
    );
}

#[test]
fn capsule_environment_roots_home_defaults_path_and_preserves_minimal_values() {
    let base = ClaudeProcessEnvironment::new(BTreeMap::from([
        ("HOME".to_owned(), "/host/home".to_owned()),
        ("LANG".to_owned(), "C.UTF-8".to_owned()),
    ]))
    .assert_value();
    let derived = base
        .for_capsule(Path::new("/capsule/runtime"), "/configured/bin")
        .assert_value();
    assert_eq!(
        derived.clone_values(),
        BTreeMap::from([
            ("HOME".to_owned(), "/capsule/runtime".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("PATH".to_owned(), "/configured/bin".to_owned()),
        ])
    );

    let explicit_path = ClaudeProcessEnvironment::new(BTreeMap::from([(
        "PATH".to_owned(),
        "/explicit/bin".to_owned(),
    )]))
    .assert_value();
    assert_eq!(
        explicit_path
            .for_capsule(Path::new("/next/runtime"), "/configured/bin")
            .assert_value()
            .clone_values(),
        BTreeMap::from([
            ("HOME".to_owned(), "/next/runtime".to_owned()),
            ("PATH".to_owned(), "/explicit/bin".to_owned()),
        ])
    );
}

fn bedrock_environment(
    values: &[(&str, &str)],
) -> Result<BTreeMap<String, String>, NodeRunnerError> {
    let workspace = TestDirectory::new("claude-bedrock-environment");
    let binding = agent_binding(
        "global.anthropic.provider-owned-model",
        Some(ReasoningEffort::Max),
        SessionScope::Execution,
        &values.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
    );
    let resolved = ResolvedEnvironment::exact(
        &binding,
        values
            .iter()
            .map(|(name, value)| (environment_name(name), (*value).to_owned()))
            .collect(),
    )
    .assert_value();
    let adapter = ClaudeAdapter::new_for_test(ClaudeAdapterConfig {
        provider: ClaudeProvider::Bedrock,
        executable: "claude".to_owned(),
        prefix_arguments: Vec::new(),
        workspace: workspace.path().to_owned(),
        runtime_home: workspace.child("runtime"),
        local_user_home: None,
        base_environment: ClaudeProcessEnvironment::new(BTreeMap::from([(
            "PATH".to_owned(),
            "/usr/bin:/bin".to_owned(),
        )]))
        .assert_value(),
        process_pool: HostedProcessPool::new(10_002, 10_002, 20_000, 20_000).assert_value(),
    })
    .assert_value();
    adapter.process_environment(&resolved, Path::new("/private/session"))
}

#[test]
fn bedrock_environment_sets_control_and_rejects_missing_or_conflicting_values() {
    let valid = bedrock_environment(&[
        (AWS_BEARER_TOKEN_BEDROCK, "bedrock-secret"),
        (AWS_REGION, "us-east-1"),
    ])
    .assert_value();
    assert_eq!(
        valid.get(AWS_BEARER_TOKEN_BEDROCK).map(String::as_str),
        Some("bedrock-secret")
    );
    assert_eq!(valid.get(AWS_REGION).map(String::as_str), Some("us-east-1"));
    assert_eq!(
        valid.get("CLAUDE_CODE_USE_BEDROCK").map(String::as_str),
        Some("1")
    );

    for values in [
        vec![(AWS_REGION, "us-east-1")],
        vec![(AWS_BEARER_TOKEN_BEDROCK, "bedrock-secret")],
        vec![(AWS_BEARER_TOKEN_BEDROCK, ""), (AWS_REGION, "us-east-1")],
        vec![
            (AWS_BEARER_TOKEN_BEDROCK, "bedrock-secret"),
            (AWS_REGION, ""),
        ],
    ] {
        assert!(bedrock_environment(&values).is_err(), "accepted {values:?}");
    }

    for conflict in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "OPENROUTER_API_KEY",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_BEDROCK_BASE_URL",
        "ANTHROPIC_BEDROCK_MANTLE_BASE_URL",
        "CLAUDE_CONFIG_DIR",
        "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_USE_ANTHROPIC_AWS",
        "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_GATEWAY",
        "CLAUDE_CODE_USE_MANTLE",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
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
