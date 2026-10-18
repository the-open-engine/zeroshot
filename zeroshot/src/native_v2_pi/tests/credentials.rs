//! Credential and Bedrock tests for the Pi lane.
//!
//! A child module of `native_v2_pi/tests.rs`, so the shared fixtures stay in one place and
//! the coverage totals keep excluding this file (`AGENTS.md`).

use super::*;

#[test]
fn declaring_any_anthropic_authentication_form_suppresses_other_ambient_forms() {
    let names = [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_OAUTH_TOKEN",
    ];
    for declared in names {
        let directory = TestDirectory::new("pi-auth-precedence");
        let files = files(&directory);
        let ambient = names
            .into_iter()
            .map(|name| (name.to_owned(), "ambient".to_owned()))
            .collect();
        let adapter =
            PiAdapter::new_local(adapter_configuration(PiProvider::Anthropic, "pi", ambient))
                .assert_value();
        let invocation = invocation(
            agent_binding("provider-model", None, SessionScope::Execution, &[declared]),
            NodeRole::Worker,
            resolved(&[declared], &[(declared, "declared-value")]),
        );
        let command = adapter
            .command_for_test(&invocation, &files, "zs-fixed")
            .assert_value();
        assert_eq!(
            command.environment.get(declared).map(String::as_str),
            Some("declared-value")
        );
        for other in names.into_iter().filter(|name| *name != declared) {
            assert!(
                !command.environment.contains_key(other),
                "{other} must not override {declared}"
            );
        }
    }
}

#[test]
fn coverage_contract_pi_foreign_lane_credentials_are_refused() {
    // A credential from another lane must never authenticate the selected provider.
    for (provider, foreign) in [
        (PiProvider::Anthropic, "OPENAI_API_KEY"),
        (PiProvider::OpenAi, "ANTHROPIC_API_KEY"),
        (PiProvider::OpenRouter, "ANTHROPIC_API_KEY"),
    ] {
        let directory = TestDirectory::new("pi-foreign");
        let files = files(&directory);
        let command = super::super::command::command(PiCommandRequest {
            provider,
            native_local: false,
            contained: false,
            executable: "pi",
            prefix_arguments: &[],
            invocation: &invocation(
                agent_binding("provider/model", None, SessionScope::Execution, &[foreign]),
                NodeRole::Worker,
                resolved(&[foreign], &[(foreign, "smuggled")]),
            ),
            files: &files,
            local_environment: &BTreeMap::new(),
            agent_dir: directory.path(),
            session_dir: &directory.child("sessions"),
            session_id: "zs-fixed",
        });
        assert!(command.is_err(), "{foreign} must not satisfy {provider:?}");
    }
}

#[test]
fn coverage_contract_pi_bedrock_pair_is_foreign_to_every_non_bedrock_lane() {
    // A Bedrock credential or region must not ride along on a lane that authenticates elsewhere.
    for (provider, accepted) in [
        (
            PiProvider::Anthropic,
            ("ANTHROPIC_API_KEY", "anthropic-key"),
        ),
        (PiProvider::OpenAi, ("OPENAI_API_KEY", "openai-key")),
        (
            PiProvider::OpenRouter,
            ("OPENROUTER_API_KEY", "openrouter-key"),
        ),
    ] {
        for foreign in ["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"] {
            let directory = TestDirectory::new("pi-bedrock-foreign");
            let files = files(&directory);
            let command = super::super::command::command(PiCommandRequest {
                provider,
                native_local: false,
                contained: false,
                executable: "pi",
                prefix_arguments: &[],
                invocation: &invocation(
                    agent_binding(
                        "provider/model",
                        None,
                        SessionScope::Execution,
                        &[accepted.0, foreign],
                    ),
                    NodeRole::Worker,
                    resolved(
                        &[accepted.0, foreign],
                        &[(accepted.0, accepted.1), (foreign, "foreign")],
                    ),
                ),
                files: &files,
                local_environment: &BTreeMap::new(),
                agent_dir: directory.path(),
                session_dir: &directory.child("sessions"),
                session_id: "zs-fixed",
            });
            assert!(
                command.is_err(),
                "{foreign} must not ride along on {provider:?}"
            );
        }
    }
}

#[test]
fn coverage_contract_pi_gateway_refuses_the_bedrock_pair() {
    for foreign in ["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"] {
        let directory = TestDirectory::new("pi-gateway-bedrock");
        let files = files(&directory);
        let fields = [
            "GATEWAY_BASE_URL",
            "GATEWAY_API_KEY",
            "GATEWAY_API",
            foreign,
        ];
        let command = super::super::command::command(PiCommandRequest {
            provider: PiProvider::Gateway,
            native_local: false,
            contained: false,
            executable: "pi",
            prefix_arguments: &[],
            invocation: &invocation(
                agent_binding("provider/model", None, SessionScope::Execution, &fields),
                NodeRole::Worker,
                resolved(
                    &fields,
                    &[
                        ("GATEWAY_BASE_URL", "https://gateway.example/api/v1"),
                        ("GATEWAY_API_KEY", "sk-gateway"),
                        ("GATEWAY_API", "openai-completions"),
                        (foreign, "foreign"),
                    ],
                ),
            ),
            files: &files,
            local_environment: &BTreeMap::new(),
            agent_dir: directory.path(),
            session_dir: &directory.child("sessions"),
            session_id: "zs-fixed",
        });
        assert!(
            command.is_err(),
            "{foreign} must not reach the gateway lane"
        );
    }
}

#[test]
fn coverage_contract_pi_present_but_empty_declared_credential_fails_closed() {
    for (provider, field) in [
        (PiProvider::Anthropic, "ANTHROPIC_API_KEY"),
        (PiProvider::OpenAi, "OPENAI_API_KEY"),
        (PiProvider::OpenRouter, "OPENROUTER_API_KEY"),
    ] {
        let directory = TestDirectory::new("pi-empty");
        let files = files(&directory);
        let command = super::super::command::command(PiCommandRequest {
            provider,
            native_local: false,
            contained: false,
            executable: "pi",
            prefix_arguments: &[],
            invocation: &invocation(
                agent_binding("provider/model", None, SessionScope::Execution, &[field]),
                NodeRole::Worker,
                resolved(&[field], &[(field, "")]),
            ),
            files: &files,
            local_environment: &BTreeMap::new(),
            agent_dir: directory.path(),
            session_dir: &directory.child("sessions"),
            session_id: "zs-fixed",
        });
        assert!(
            command.is_err(),
            "an empty declared {field} must not fall back to a stored login"
        );
    }
}

#[test]
fn coverage_contract_pi_bedrock_lane_requires_its_own_pair() {
    let directory = TestDirectory::new("pi-bedrock");
    let files = files(&directory);
    let complete = super::super::command::command(PiCommandRequest {
        provider: PiProvider::Bedrock,
        native_local: false,
        contained: false,
        executable: "pi",
        prefix_arguments: &[],
        invocation: &invocation(
            agent_binding(
                "amazon-bedrock/anthropic.claude",
                None,
                SessionScope::Execution,
                &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
            ),
            NodeRole::Worker,
            resolved(
                &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
                &[
                    ("AWS_BEARER_TOKEN_BEDROCK", "bedrock-token"),
                    ("AWS_REGION", "us-east-1"),
                ],
            ),
        ),
        files: &files,
        local_environment: &BTreeMap::new(),
        agent_dir: directory.path(),
        session_dir: &directory.child("sessions"),
        session_id: "zs-fixed",
    })
    .assert_value_with("bedrock launch");
    assert_eq!(
        value(&complete.argv, "--provider").as_deref(),
        Some("amazon-bedrock")
    );

    // Only one half of the pair is not enough.
    let partial = super::super::command::command(PiCommandRequest {
        provider: PiProvider::Bedrock,
        native_local: false,
        contained: false,
        executable: "pi",
        prefix_arguments: &[],
        invocation: &invocation(
            agent_binding(
                "amazon-bedrock/anthropic.claude",
                None,
                SessionScope::Execution,
                &["AWS_REGION"],
            ),
            NodeRole::Worker,
            resolved(&["AWS_REGION"], &[("AWS_REGION", "us-east-1")]),
        ),
        files: &files,
        local_environment: &BTreeMap::new(),
        agent_dir: directory.path(),
        session_dir: &directory.child("sessions"),
        session_id: "zs-fixed",
    });
    assert!(partial.is_err());
}

#[test]
fn coverage_contract_pi_bedrock_lane_rejects_a_present_but_empty_pair_member() {
    // A present-but-empty region must fail closed instead of being treated as an absent pair half.
    let directory = TestDirectory::new("pi-bedrock-empty-region");
    let files = files(&directory);
    let command = super::super::command::command(PiCommandRequest {
        provider: PiProvider::Bedrock,
        native_local: false,
        contained: false,
        executable: "pi",
        prefix_arguments: &[],
        invocation: &invocation(
            agent_binding(
                "amazon-bedrock/anthropic.claude",
                None,
                SessionScope::Execution,
                &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
            ),
            NodeRole::Worker,
            resolved(
                &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
                &[
                    ("AWS_BEARER_TOKEN_BEDROCK", "bedrock-token"),
                    ("AWS_REGION", ""),
                ],
            ),
        ),
        files: &files,
        local_environment: &BTreeMap::new(),
        agent_dir: directory.path(),
        session_dir: &directory.child("sessions"),
        session_id: "zs-fixed",
    });
    assert!(
        command.is_err(),
        "an empty AWS_REGION must not be treated as an absent pair half"
    );
}
