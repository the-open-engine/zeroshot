use super::*;
use crate::harnesses::{HarnessBuildContext, HarnessHost, HarnessRegistry};
use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;

async fn selected(
    provider: CopilotProvider,
    declared: &[(&str, &str)],
    ambient: BTreeMap<String, String>,
) -> Fixture {
    let directory = TestDirectory::new("copilot-explicit-provider");
    let executable = directory.write_executable("copilot", &script());
    let mut values = fixture_values(&directory, "explicit_provider");
    values.extend(
        declared
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    let binding = binding(values.keys().cloned());
    let admitted = admitted_for(binding.clone(), INSTRUCTIONS, provider).await;
    let workspace = directory.child("workspace");
    std::fs::create_dir_all(&workspace).assert_value();
    std::fs::create_dir_all(directory.child("runtime")).assert_value();
    let context = HarnessBuildContext {
        lane: crate::native_v2_contract::RuntimeLane::new("copilot", provider.name())
            .assert_value(),
        workspace,
        runtime_home: directory.child("runtime"),
        executable,
        prefix_arguments: Vec::new(),
        search_path: "/usr/bin:/bin".to_owned(),
        base_environment: BTreeMap::new(),
        host: HarnessHost::Local {
            home: Some(directory.child("user-home")),
            environment: LocalHarnessEnvironment::new(ambient),
        },
    };
    let adapter = HarnessRegistry::core()
        .configure(context)
        .assert_value()
        .build(None)
        .assert_value();
    let runtime = NativeNodeRunner::new(&admitted, adapter.driver, adapter.sessions).assert_value();
    Fixture {
        directory,
        runtime,
        binding,
        instructions: INSTRUCTIONS,
        values,
    }
}

#[tokio::test]
async fn explicit_provider_lanes_use_native_rpc_and_keep_sessions() {
    for (provider, key, kind, url, wire) in [
        (
            CopilotProvider::OpenAi,
            "OPENAI_API_KEY",
            "openai",
            "https://api.openai.com/v1",
            Some("responses"),
        ),
        (
            CopilotProvider::Anthropic,
            "ANTHROPIC_API_KEY",
            "anthropic",
            "https://api.anthropic.com",
            None,
        ),
        (
            CopilotProvider::OpenRouter,
            "OPENROUTER_API_KEY",
            "openai",
            "https://openrouter.ai/api/v1",
            Some("completions"),
        ),
    ] {
        let fixture = selected(
            provider,
            &[(key, "declared-provider-secret")],
            BTreeMap::new(),
        )
        .await;
        verified(complete(fixture.start(1).await).await.1);
        verified(complete(fixture.start(2).await).await.1);
        let sessions: Vec<_> = fixture
            .capture()
            .into_iter()
            .filter(|m| {
                matches!(
                    m["method"].as_str(),
                    Some("session.create" | "session.resume")
                )
            })
            .collect();
        assert_eq!(sessions.len(), 2);
        for session in sessions {
            let config = &session["params"]["provider"];
            assert_eq!(config["apiKey"], "declared-provider-secret");
            assert_eq!(config["type"], kind);
            assert_eq!(config["baseUrl"], url);
            assert_eq!(config["wireApi"].as_str(), wire);
        }
    }
}

#[tokio::test]
async fn explicit_provider_authentication_survives_native_grammar_retry() {
    let mut fixture = selected(
        CopilotProvider::OpenAi,
        &[("OPENAI_API_KEY", "declared-provider-secret")],
        BTreeMap::from([(
            "COPILOT_GITHUB_TOKEN".to_owned(),
            "wrong-native-github-token".to_owned(),
        )]),
    )
    .await;
    fixture.values.insert(
        "TEST_MODE".to_owned(),
        "explicit_provider_grammar_once".to_owned(),
    );
    verified(complete(fixture.start(1).await).await.1);
    let capture = fixture.capture();
    assert_eq!(method_count(&capture, "session.create"), 1);
    assert_eq!(method_count(&capture, "session.resume"), 1);
    for message in capture.iter().filter(|message| {
        matches!(
            message["method"].as_str(),
            Some("session.create" | "session.resume")
        )
    }) {
        let params = &message["params"];
        assert_eq!(params["provider"]["apiKey"], "declared-provider-secret");
        assert!(params.get("gitHubToken").is_none());
        assert!(params.get("gitHubTokenProviderRegistrationId").is_none());
    }
}

#[tokio::test]
async fn gateway_requires_explicit_protocol_and_preserves_authored_path() {
    for (api, kind, wire) in [
        ("openai-responses", "openai", Some("responses")),
        ("openai-completions", "openai", Some("completions")),
        ("anthropic-messages", "anthropic", None),
    ] {
        let fixture = selected(
            CopilotProvider::Gateway,
            &[
                ("GATEWAY_API_KEY", "gateway-secret"),
                ("GATEWAY_BASE_URL", "https://proxy.example/owned/path"),
                ("GATEWAY_API", api),
            ],
            BTreeMap::new(),
        )
        .await;
        verified(complete(fixture.start(1).await).await.1);
        let capture = fixture.capture();
        let config = created_provider(&capture);
        assert_eq!(config["type"], kind);
        assert_eq!(config["wireApi"].as_str(), wire);
        assert_eq!(config["baseUrl"], "https://proxy.example/owned/path");
    }
    let fixture = selected(
        CopilotProvider::Gateway,
        &[
            ("GATEWAY_API_KEY", "secret"),
            ("GATEWAY_BASE_URL", "https://proxy.example"),
        ],
        BTreeMap::new(),
    )
    .await;
    rejected_before_launch(&fixture).await;
}

#[tokio::test]
async fn explicit_provider_credentials_override_native_auth_but_preserve_settings() {
    let mut native = custom_provider_environment();
    native.insert(
        "COPILOT_PROVIDER_HEADERS".to_owned(),
        "Authorization: old-auth\\nCookie: native-cookie\\nX-Tenant: alpha".to_owned(),
    );
    let fixture = selected(
        CopilotProvider::OpenAi,
        &[("OPENAI_API_KEY", "new-secret")],
        native,
    )
    .await;
    verified(complete(fixture.start(1).await).await.1);
    let capture = fixture.capture();
    let config = created_provider(&capture);
    assert_eq!(config["baseUrl"], "https://gateway.example/v1");
    assert_eq!(config["apiKey"], "new-secret");
    assert_eq!(config["wireModel"], "gateway-model");
    assert_eq!(config["maxOutputTokens"], 4096);
    assert_eq!(config["headers"]["Cookie"], "native-cookie");
    assert_eq!(config["headers"]["X-Tenant"], "alpha");
    assert!(config["headers"].get("Authorization").is_none());
}

#[tokio::test]
async fn foreign_credentials_and_endpoint_changes_cannot_reuse_stored_auth() {
    for declared in [
        vec![("ANTHROPIC_API_KEY", "foreign-secret")],
        vec![
            ("COPILOT_GITHUB_TOKEN", "foreign-github-secret"),
            ("OPENAI_API_KEY", "selected-secret"),
        ],
        vec![("OPENAI_BASE_URL", "https://different.example/v1")],
    ] {
        let fixture = selected(
            CopilotProvider::OpenAi,
            &declared,
            custom_provider_environment(),
        )
        .await;
        rejected_before_launch(&fixture).await;
    }
}

fn created_provider(capture: &[Value]) -> &Value {
    &capture
        .iter()
        .find(|m| m["method"] == "session.create")
        .assert_value()["params"]["provider"]
}

#[tokio::test]
async fn canonical_shell_context_is_private_and_declared_credentials_win() {
    let ambient = BTreeMap::from([
        ("OPENAI_API_KEY".to_owned(), "shell-provider-key".to_owned()),
        (
            "OPENAI_BASE_URL".to_owned(),
            "https://shell.example/v1".to_owned(),
        ),
        (
            "ANTHROPIC_API_KEY".to_owned(),
            "foreign-shell-key".to_owned(),
        ),
    ]);
    for declared in [vec![], vec![("OPENAI_API_KEY", "declared-provider-key")]] {
        let fixture = selected(CopilotProvider::OpenAi, &declared, ambient.clone()).await;
        verified(complete(fixture.start(1).await).await.1);
        let capture = fixture.capture();
        let config = created_provider(&capture);
        assert_eq!(config["baseUrl"], "https://shell.example/v1");
        assert_eq!(
            config["apiKey"],
            declared
                .first()
                .map_or("shell-provider-key", |(_, key)| *key)
        );
        assert!(
            !capture
                .iter()
                .any(|message| message.to_string().contains("foreign-shell-key"))
        );
    }
}

#[tokio::test]
async fn changed_endpoint_drops_stored_cookies_and_other_native_headers() {
    let mut native = custom_provider_environment();
    native.insert(
        "COPILOT_PROVIDER_HEADERS".to_owned(),
        "Cookie: stored-cookie\\nX-Tenant: old-tenant".to_owned(),
    );
    let fixture = selected(
        CopilotProvider::OpenAi,
        &[
            ("OPENAI_API_KEY", "new-key"),
            ("OPENAI_BASE_URL", "https://new.example/v1"),
        ],
        native,
    )
    .await;
    verified(complete(fixture.start(1).await).await.1);
    assert!(
        created_provider(&fixture.capture())
            .get("headers")
            .is_none()
    );
}

#[tokio::test]
async fn bedrock_uses_native_responses_byok_with_the_selected_region() {
    let fixture = selected(
        CopilotProvider::Bedrock,
        &[
            ("AWS_BEARER_TOKEN_BEDROCK", "aws-provider-secret"),
            ("AWS_REGION", "us-east-1"),
        ],
        BTreeMap::new(),
    )
    .await;
    verified(complete(fixture.start(1).await).await.1);
    let capture = fixture.capture();
    let config = created_provider(&capture);
    assert_eq!(config["apiKey"], "aws-provider-secret");
    assert_eq!(
        config["baseUrl"],
        "https://bedrock-runtime.us-east-1.amazonaws.com/openai/v1"
    );
    assert_eq!(config["wireApi"], "responses");
}

#[tokio::test]
async fn declared_endpoint_alias_replaces_all_ambient_aliases() {
    let fixture = selected(
        CopilotProvider::OpenAi,
        &[
            ("OPENAI_API_KEY", "new-key"),
            ("OPENAI_API_BASE", "https://new.example/v1"),
        ],
        BTreeMap::from([(
            "OPENAI_BASE_URL".to_owned(),
            "https://old.example/v1".to_owned(),
        )]),
    )
    .await;
    verified(complete(fixture.start(1).await).await.1);
    assert_eq!(
        created_provider(&fixture.capture())["baseUrl"],
        "https://new.example/v1"
    );
}

#[test]
fn explicit_headers_replace_registry_headers_when_changing_endpoint() {
    let config = registry_configuration(
        Some("https://old.example/v1"),
        Some("X-Tenant: declared-tenant"),
    );
    assert_eq!(config["headers"], json!({"X-Tenant":"declared-tenant"}));
}

#[test]
fn an_unknown_native_endpoint_cannot_forward_stored_cookies() {
    let config = registry_configuration(None, None);
    assert!(config.get("headers").is_none());
    assert_eq!(config["baseUrl"], "https://new.example/v1");
}

fn registry_configuration(base: Option<&str>, headers: Option<&str>) -> Value {
    let mut values = vec![
        ("OPENAI_API_KEY", "new-key"),
        ("OPENAI_BASE_URL", "https://new.example/v1"),
    ];
    if let Some(headers) = headers {
        values.push(("COPILOT_PROVIDER_HEADERS", headers));
    }
    let selected = selected_registry(CopilotProvider::OpenAi, json!({"name":"native","type":"openai","baseUrl":base,"apiKey":"stored-key","headers":{"Cookie":"stored-cookie"}}), &values).assert_value();
    let mut parameters = json!({});
    selected.configure(&mut parameters);
    parameters["provider"].take()
}

#[test]
fn incomplete_native_routes_cannot_retarget_stored_provider_credentials() {
    let azure = json!({"name":"native","type":"azure","apiKey":"stored-azure-key"});
    assert!(selected_registry(CopilotProvider::OpenAi, azure, &[]).is_err());
    let openai = json!({"name":"native","type":"openai","apiKey":"stored-openai-key"});
    assert!(
        selected_registry(
            CopilotProvider::Bedrock,
            openai,
            &[("AWS_REGION", "us-east-1")]
        )
        .is_err()
    );
}

fn selected_registry(
    selected_provider: CopilotProvider,
    stored: Value,
    fields: &[(&str, &str)],
) -> Result<provider::LocalProvider, NodeRunnerError> {
    let directory = TestDirectory::new("copilot-registry-canonical-headers");
    let path = directory.write(
        "providers.json",
        &json!({"providers":[stored],"models":[{"provider":"native","id":"model"}]}).to_string(),
    );
    let context = provider::take_local_configuration(
        &mut BTreeMap::from([(
            "COPILOT_PROVIDERS_CONFIG".to_owned(),
            path.to_string_lossy().into_owned(),
        )]),
        directory.path(),
    );
    let mut values = BTreeMap::from([(
        environment_name("TEST_MODE"),
        "explicit_provider".to_owned(),
    )]);
    values.extend(
        fields
            .iter()
            .map(|(name, value)| (environment_name(name), (*value).to_owned())),
    );
    let binding = binding(values.keys().map(|name| name.as_str().to_owned()));
    let resolved = ResolvedEnvironment::exact(&binding, values).assert_value();
    provider::resolve_explicit_configuration(
        selected_provider,
        Some(&context),
        &resolved,
        "native/model",
    )
}
