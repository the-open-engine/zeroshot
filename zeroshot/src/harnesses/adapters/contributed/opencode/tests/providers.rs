use super::*;
use crate::execution::process::{HostedProcessScope, ProcessSessionCommand};
use crate::native_v2_candidate::test_support::TestDirectory;

fn prepared(
    directory: &TestDirectory,
    provider: &str,
    fields: &[(&str, &str)],
) -> (ProviderExecutionFiles, ProcessSessionCommand) {
    prepared_with_config(directory, provider, fields, None)
}

fn prepared_with_config(
    directory: &TestDirectory,
    provider: &str,
    fields: &[(&str, &str)],
    inline: Option<Value>,
) -> (ProviderExecutionFiles, ProcessSessionCommand) {
    let mut adapter = super::process::adapter(directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = provider.to_owned();
    if let Some(inline) = inline {
        adapter
            .local_environment
            .insert("OPENCODE_CONFIG_CONTENT".to_owned(), inline.to_string());
    }
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let command = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &super::process::invocation(fields),
        files: &files,
        session_id: None,
    })
    .assert_value();
    (files, command)
}

#[test]
fn gateway_selects_native_sdk_and_preserves_native_model_settings() {
    for api in [
        "openai-responses",
        "openai-completions",
        "anthropic-messages",
    ] {
        let directory = TestDirectory::new("opencode-gateway-native");
        let (_files, command) = prepared(
            &directory,
            "gateway",
            &[
                ("GATEWAY_API_KEY", "selected-secret"),
                ("GATEWAY_BASE_URL", "https://proxy.example/authored/path"),
                ("GATEWAY_API", api),
            ],
        );
        assert!(command.argv.iter().any(|arg| arg == "gateway/opaque/model"));
        let config: Value =
            serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
        let provider = &config["provider"]["gateway"];
        assert_eq!(
            provider["npm"],
            match api {
                "openai-responses" => "@ai-sdk/openai",
                "openai-completions" => "@ai-sdk/openai-compatible",
                _ => "@ai-sdk/anthropic",
            }
        );
        assert_eq!(
            provider["models"]["opaque/model"],
            json!({"name":"opaque/model"})
        );
        assert_eq!(provider["options"]["apiKey"], "{env:GATEWAY_API_KEY}");
        assert_eq!(
            provider["options"]["baseURL"],
            "https://proxy.example/authored/path"
        );
        owned_plugin(&command, "zeroshot-provider-auth");

        let directory = TestDirectory::new("opencode-gateway-native-preserved");
        let (_files, command) = prepared_with_config(
            &directory,
            "gateway",
            &[
                ("GATEWAY_API_KEY", "selected-secret"),
                ("GATEWAY_BASE_URL", "https://proxy.example/authored/path"),
                ("GATEWAY_API", api),
            ],
            Some(
                json!({"provider":{"gateway":{"models":{"opaque/model":{"limit":{"context":98765}}}}}}),
            ),
        );
        let config: Value =
            serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
        let model = &config["provider"]["gateway"]["models"]["opaque/model"];
        assert_eq!(model["limit"]["context"], 98765);
        assert!(model.get("name").is_none());
    }
}

#[test]
fn bedrock_pins_bearer_region_and_native_endpoint_without_replacing_other_settings() {
    let directory = TestDirectory::new("opencode-bedrock-native");
    let (_files, command) = prepared(
        &directory,
        "bedrock",
        &[
            ("AWS_BEARER_TOKEN_BEDROCK", "selected-secret"),
            ("AWS_REGION", "us-east-1"),
            (
                "AWS_BEDROCK_BASE_URL",
                "https://bedrock.example/native/path",
            ),
        ],
    );
    assert!(
        command
            .argv
            .iter()
            .any(|arg| arg == "amazon-bedrock/opaque/model")
    );
    let config: Value =
        serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    let options = &config["provider"]["amazon-bedrock"]["options"];
    assert_eq!(options["apiKey"], "{env:AWS_BEARER_TOKEN_BEDROCK}");
    assert_eq!(options["region"], "us-east-1");
    assert_eq!(options["baseURL"], "https://bedrock.example/native/path");
    assert_eq!(options["endpoint"], "https://bedrock.example/native/path");
    owned_plugin(&command, "zeroshot-provider-auth");
}

fn ambient_adapter(directory: &TestDirectory, provider: &str, body: &str) -> OpenCodeAdapter {
    let mut config = super::process::adapter(directory, body).config;
    config.provider = provider.to_owned();
    config.native_environment = LocalHarnessEnvironment::new(BTreeMap::from([
        (
            "AWS_BEARER_TOKEN_BEDROCK".to_owned(),
            "synthetic-bearer-secret".to_owned(),
        ),
        (
            "AWS_SECRET_ACCESS_KEY".to_owned(),
            "synthetic-iam-secret".to_owned(),
        ),
        ("AWS_PROFILE".to_owned(), "native-profile".to_owned()),
        (
            "AWS_WEB_IDENTITY_TOKEN_FILE".to_owned(),
            "/native/web-token".to_owned(),
        ),
    ]));
    OpenCodeAdapter::new_local(config).assert_value()
}

#[test]
fn native_aws_authentication_is_preserved_only_for_the_bedrock_lane() {
    for provider in ["openai", "bedrock"] {
        let directory = TestDirectory::new("opencode-aws-context");
        let adapter = ambient_adapter(&directory, provider, "#!/bin/sh\nexit 0\n");
        if provider == "bedrock" {
            assert_eq!(adapter.local_environment["AWS_PROFILE"], "native-profile");
            assert_eq!(
                adapter.local_environment["AWS_WEB_IDENTITY_TOKEN_FILE"],
                "/native/web-token"
            );
        } else {
            assert!(
                !adapter
                    .local_environment
                    .keys()
                    .any(|name| name.starts_with("AWS_"))
            );
        }
    }
}

#[tokio::test]
async fn native_aws_errors_redact_ambient_bearer_and_iam_secrets() {
    let directory = TestDirectory::new("opencode-aws-redaction");
    let adapter = ambient_adapter(
        &directory,
        "bedrock",
        "#!/bin/sh\nif [ \"$1\" = auth ]; then printf '[]\\n'; exit 0; fi\ncat >/dev/null\nprintf '%s\\n' \"$AWS_BEARER_TOKEN_BEDROCK $AWS_SECRET_ACCESS_KEY\" >&2\nexit 1\n",
    );
    let error = super::process::run(&adapter, super::process::invocation(&[]))
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("synthetic-bearer-secret"));
    assert!(!error.contains("synthetic-iam-secret"));
    assert!(error.contains("[REDACTED]"));
}

fn provider_config(command: &ProcessSessionCommand, provider: &str) -> Option<Value> {
    let config: Value =
        serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    config.get("provider")?.get(provider).cloned()
}

#[test]
fn opencode_zen_and_go_reuse_native_login_with_native_provider_ids() {
    for provider in ["opencode", "opencode-go"] {
        let directory = TestDirectory::new("opencode-account-native");
        let (_files, command) = prepared(&directory, provider, &[]);
        assert!(
            command
                .argv
                .iter()
                .any(|arg| *arg == format!("{provider}/opaque/model"))
        );
        assert!(provider_config(&command, provider).is_none());
        assert!(!command.environment.contains_key("OPENCODE_AUTH_CONTENT"));
    }
}

#[test]
fn opencode_zen_and_go_apply_a_declared_account_key_to_their_own_provider() {
    for provider in ["opencode", "opencode-go"] {
        let directory = TestDirectory::new("opencode-account-declared");
        let (_files, command) = prepared(
            &directory,
            provider,
            &[("OPENCODE_API_KEY", "selected-secret")],
        );
        assert!(
            command
                .argv
                .iter()
                .any(|arg| *arg == format!("{provider}/opaque/model"))
        );
        assert_eq!(
            provider_config(&command, provider).assert_value()["options"]["apiKey"],
            "{env:OPENCODE_API_KEY}"
        );
        owned_plugin(&command, "zeroshot-provider-auth");
    }
}

#[test]
fn opencode_account_key_is_declarable_while_other_native_controls_stay_reserved() {
    command::validate_connection_name("OPENCODE_API_KEY", false).assert_value();
    for reserved in [
        "OPENCODE_CONFIG",
        "OPENCODE_AUTH_CONTENT",
        "OPENCODE_PURE",
        "OPENCODE_PERMISSION",
    ] {
        assert!(command::validate_connection_name(reserved, false).is_err());
    }
}

#[test]
fn opencode_account_key_is_foreign_to_other_lanes() {
    let directory = TestDirectory::new("opencode-account-foreign");
    let mut adapter = super::process::adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "openai".to_owned();
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let error = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &super::process::invocation(&[("OPENCODE_API_KEY", "foreign-secret")]),
        files: &files,
        session_id: None,
    })
    .unwrap_err();
    assert!(error.to_string().contains("another provider's credential"));
}

#[test]
fn ambient_opencode_account_key_reaches_only_the_opencode_lanes() {
    for (provider, inherited) in [("opencode-go", true), ("opencode", true), ("openai", false)] {
        let directory = TestDirectory::new("opencode-account-ambient");
        let mut config = super::process::adapter(&directory, "#!/bin/sh\nexit 0\n").config;
        config.provider = provider.to_owned();
        config.native_environment = LocalHarnessEnvironment::new(BTreeMap::from([(
            "OPENCODE_API_KEY".to_owned(),
            "ambient-account-secret".to_owned(),
        )]));
        let adapter = OpenCodeAdapter::new_local(config).assert_value();
        let files = ProviderExecutionFiles::for_test(
            directory.path(),
            HostedProcessScope::WriterExecution(1),
        )
        .assert_value();
        let command = command::command(command::CommandRequest {
            adapter: &adapter,
            invocation: &super::process::invocation(&[]),
            files: &files,
            session_id: None,
        })
        .assert_value();
        assert_eq!(
            command.environment.contains_key("OPENCODE_API_KEY"),
            inherited,
            "{provider}"
        );
    }
}
