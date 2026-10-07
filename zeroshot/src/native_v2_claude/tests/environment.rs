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

pub(super) fn provider_environment(
    provider: ClaudeProvider,
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
    let adapter = ClaudeAdapter::new(ClaudeAdapterConfig {
        provider,
        executable: "claude".to_owned(),
        prefix_arguments: Vec::new(),
        workspace: workspace.path().to_owned(),
        runtime_home: workspace.child("runtime"),
        local_user_home: None,
        native_environment: Default::default(),
        base_environment: ClaudeProcessEnvironment::new(BTreeMap::from([(
            "PATH".to_owned(),
            "/usr/bin:/bin".to_owned(),
        )]))
        .assert_value(),
        process_pool: HostedProcessPool::new(10_002, 10_002, 20_000).assert_value(),
    })
    .assert_value();
    adapter.process_environment(&resolved, Path::new("/private/session"))
}

#[test]
fn bedrock_environment_sets_control_and_rejects_missing_or_conflicting_values() {
    let valid = provider_environment(
        ClaudeProvider::Bedrock,
        &[
            (AWS_BEARER_TOKEN_BEDROCK, "bedrock-secret"),
            (AWS_REGION, "us-east-1"),
        ],
    )
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
        assert!(
            provider_environment(ClaudeProvider::Bedrock, &values).is_err(),
            "accepted {values:?}"
        );
    }

    for conflict in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "OPENROUTER_API_KEY",
        "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
    ] {
        let values = [
            (AWS_BEARER_TOKEN_BEDROCK, "bedrock-secret"),
            (AWS_REGION, "us-east-1"),
            (conflict, "conflict"),
        ];
        assert!(
            provider_environment(ClaudeProvider::Bedrock, &values).is_err(),
            "accepted conflicting {conflict}"
        );
    }
}

#[test]
fn gateway_environment_rejects_conflicting_provider_credentials() {
    let required = [
        ("GATEWAY_BASE_URL", "https://gateway.example/api"),
        ("GATEWAY_API_KEY", "gateway-secret"),
    ];
    assert!(provider_environment(ClaudeProvider::Gateway, &required).is_ok());
    for conflict in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
        "OPENROUTER_API_KEY",
        "AWS_BEARER_TOKEN_BEDROCK",
    ] {
        let mut values = required.to_vec();
        values.push((conflict, "conflict"));
        assert!(
            provider_environment(ClaudeProvider::Gateway, &values).is_err(),
            "accepted {conflict}"
        );
    }
}

#[test]
fn hosted_claude_passes_declared_endpoints_configuration_and_controls() {
    for name in crate::native_v2_capsule::provider_process::CLAUDE_LOCAL_ENVIRONMENT {
        let values = provider_environment(ClaudeProvider::Anthropic, &[(name, "caller-value")])
            .assert_value();
        assert_eq!(values.get(*name).map(String::as_str), Some("caller-value"));
    }
    let openrouter = provider_environment(
        ClaudeProvider::OpenRouter,
        &[
            ("OPENROUTER_API_KEY", "router-secret"),
            ("ANTHROPIC_BASE_URL", "https://proxy.example/anthropic"),
        ],
    )
    .assert_value();
    assert_eq!(
        openrouter["ANTHROPIC_BASE_URL"],
        "https://proxy.example/anthropic"
    );
    let bedrock = provider_environment(
        ClaudeProvider::Bedrock,
        &[
            (AWS_BEARER_TOKEN_BEDROCK, "bedrock-secret"),
            (AWS_REGION, "us-east-1"),
            (
                "ANTHROPIC_BEDROCK_BASE_URL",
                "https://proxy.example/bedrock",
            ),
            ("CLAUDE_CONFIG_DIR", "/private/config"),
        ],
    )
    .assert_value();
    assert_eq!(
        bedrock["ANTHROPIC_BEDROCK_BASE_URL"],
        "https://proxy.example/bedrock"
    );
    assert_eq!(bedrock["CLAUDE_CONFIG_DIR"], "/private/config");
    let gateway = provider_environment(
        ClaudeProvider::Gateway,
        &[
            ("GATEWAY_BASE_URL", "https://gateway.example/api"),
            ("GATEWAY_API_KEY", "gateway-secret"),
            ("ANTHROPIC_BASE_URL", "https://other.example/api"),
            ("CLAUDE_CONFIG_DIR", "/private/config"),
        ],
    )
    .assert_value();
    assert_eq!(gateway["ANTHROPIC_BASE_URL"], "https://gateway.example/api");
    assert_eq!(gateway["CLAUDE_CONFIG_DIR"], "/private/config");
}

#[test]
fn explicit_providers_reject_active_incompatible_transports_only() {
    for (provider, credentials) in [
        (
            ClaudeProvider::OpenRouter,
            vec![("OPENROUTER_API_KEY", "router-secret")],
        ),
        (
            ClaudeProvider::Gateway,
            vec![
                ("GATEWAY_BASE_URL", "https://gateway.example/api"),
                ("GATEWAY_API_KEY", "gateway-secret"),
            ],
        ),
        (
            ClaudeProvider::Bedrock,
            vec![
                (AWS_BEARER_TOKEN_BEDROCK, "bedrock-secret"),
                (AWS_REGION, "us-east-1"),
            ],
        ),
    ] {
        for name in [
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_MANTLE",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
            "CLAUDE_CODE_USE_ANTHROPIC_AWS",
            "CLAUDE_CODE_USE_ANTHROPIC_GOOGLE_CLOUD",
            "CLAUDE_CODE_USE_GATEWAY",
        ] {
            let compatible = provider == ClaudeProvider::Bedrock
                && matches!(name, "CLAUDE_CODE_USE_BEDROCK" | "CLAUDE_CODE_USE_MANTLE");
            for active in ["1", "true", "TRUE", " true ", "yes", "on"] {
                let mut values = credentials.clone();
                values.push((name, active));
                let result = provider_environment(provider, &values);
                assert_eq!(result.is_ok(), compatible, "{provider:?} {name}={active}");
            }
            for inactive in ["0", "false", ""] {
                let mut values = credentials.clone();
                values.push((name, inactive));
                assert!(provider_environment(provider, &values).is_ok());
            }
        }
    }
}
