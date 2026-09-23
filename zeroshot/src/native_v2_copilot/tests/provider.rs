use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;

#[test]
fn legacy_provider_parses_optional_protocol_fields_and_rejects_ambiguous_values() {
    let values = BTreeMap::from([
        (BASE_URL.to_owned(), "https://gateway.example/v1".to_owned()),
        (TYPE.to_owned(), "openai".to_owned()),
        (AZURE_API_VERSION.to_owned(), "2026-01-01".to_owned()),
        (MAX_PROMPT_TOKENS.to_owned(), "8192".to_owned()),
        (MAX_OUTPUT_TOKENS.to_owned(), "2048".to_owned()),
        (
            HEADERS.to_owned(),
            "X-Tenant: alpha\\nX-Route: beta".to_owned(),
        ),
    ]);
    let provider = legacy_provider(&values, "admitted/model").assert_value();
    assert_eq!(provider["baseUrl"], "https://gateway.example/v1");
    assert_eq!(provider["modelId"], "admitted/model");
    assert_eq!(provider["maxPromptTokens"], 8192);
    assert_eq!(provider["maxOutputTokens"], 2048);
    assert_eq!(provider["azure"]["apiVersion"], "2026-01-01");
    assert_eq!(provider["headers"]["X-Tenant"], "alpha");
    assert_eq!(provider["headers"]["X-Route"], "beta");

    let invalid_limit = BTreeMap::from([
        (BASE_URL.to_owned(), "https://gateway.example".to_owned()),
        (MAX_OUTPUT_TOKENS.to_owned(), "many".to_owned()),
    ]);
    assert!(legacy_provider(&invalid_limit, "model").is_err());
    for headers in ["missing-colon", ": missing-name"] {
        assert!(parse_headers(headers).is_err());
    }
}

#[test]
fn registry_resolution_preserves_capabilities_and_fails_closed_on_bad_references() {
    let registry = ProviderRegistry {
        providers: vec![json!({
            "name":"gateway", "baseUrl":"https://gateway.example/v1",
            "apiKeyCommand":"fresh-key"
        })],
        models: vec![json!({
            "provider":"gateway", "id":"fixture", "modelId":"wire-id",
            "maxPromptTokens":8192, "maxContextWindowTokens":16384,
            "maxOutputTokens":2048, "capabilities":{"vision":true}
        })],
    };
    let RegistryResolution::Active(Some(provider)) =
        resolve_active_registry(registry, "gateway/fixture", None).assert_value()
    else {
        panic!("selected registry provider must be active");
    };
    let mut params = json!({});
    provider.configure(&mut params);
    assert_eq!(params["provider"]["modelId"], "wire-id");
    assert_eq!(params["provider"]["wireModel"], "fixture");
    assert_eq!(params["provider"]["maxPromptTokens"], 8192);
    assert_eq!(params["provider"]["maxContextWindowTokens"], 16384);
    assert_eq!(params["provider"]["maxOutputTokens"], 2048);
    assert_eq!(params["provider"]["modelCapabilities"]["vision"], true);
    assert_eq!(provider.session_model("admitted"), "wire-id");
    assert!(provider.uses_api_key_command());

    let missing = ProviderRegistry {
        providers: vec![json!({"name":"other"})],
        models: vec![json!({"provider":"missing","id":"fixture"})],
    };
    assert!(resolve_active_registry(missing, "missing/fixture", None).is_err());

    let passthrough = ProviderRegistry {
        providers: vec![json!({"name":"gateway","nested":["private-value"]})],
        models: Vec::new(),
    };
    let RegistryResolution::Active(Some(provider)) =
        resolve_active_registry(passthrough, "unregistered/model", None).assert_value()
    else {
        panic!("unselected registry must remain available to Copilot");
    };
    let mut params = json!({});
    provider.configure(&mut params);
    assert_eq!(params["providers"][0]["name"], "gateway");
    assert_eq!(params["models"], json!([]));
    assert!(!provider.bypasses_github_auth());
    assert!(provider.redactions().any(|value| value == "private-value"));
}

#[test]
fn registry_files_and_offline_mode_enforce_bounded_explicit_configuration() {
    let directory = TestDirectory::new("copilot-provider-boundaries");
    let missing = directory.child("missing.json");
    assert!(matches!(
        resolve_registry(&missing, false, "model", None).assert_value(),
        RegistryResolution::Inactive
    ));
    assert!(resolve_registry(&missing, true, "model", None).is_err());
    assert!(read_registry(directory.path(), true).is_err());

    let empty = directory.child("empty.json");
    std::fs::write(&empty, "{}").assert_value();
    assert!(matches!(
        resolve_registry(&empty, true, "model", None).assert_value(),
        RegistryResolution::Inactive
    ));
    for invalid in [json!(null), json!({"providers":{}})] {
        assert!(parse_registry(invalid.to_string().as_bytes()).is_err());
    }

    let crowded = directory.child("crowded.json");
    std::fs::write(
        &crowded,
        json!({"providers":vec![json!({}); MAX_REGISTRY_ITEMS + 1]}).to_string(),
    )
    .assert_value();
    assert!(resolve_registry(&crowded, true, "model", None).is_err());
    assert!(registry_redactions(&[json!("x".repeat(MAX_REDACTION_BYTES + 1))], &[]).is_err());

    for enabled in ["1", " TRUE ", "yes", "On", "y"] {
        let environment =
            offline_environment(Some(enabled), Some("https://local.example")).assert_value();
        assert_eq!(environment[OFFLINE], enabled);
        assert_eq!(environment[BASE_URL], "https://local.example");
    }
    for disabled in [None, Some(""), Some("false"), Some("0")] {
        assert!(
            offline_environment(disabled, None)
                .assert_value()
                .is_empty()
        );
    }
    assert!(offline_environment(Some("true"), None).is_err());
}
