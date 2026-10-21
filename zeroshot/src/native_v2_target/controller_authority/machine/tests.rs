use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use super::*;

fn credential(
    key: Option<&str>,
    organization: Option<&str>,
) -> Result<Option<MachineCredential>, TargetAuthorityError> {
    MachineCredential::from_environment_with(|name| match name {
        "ZEROSHOT_API_KEY" => key.map(Into::into),
        "ZEROSHOT_ORGANIZATION" => organization.map(Into::into),
        _ => None,
    })
}

#[test]
fn missing_key_preserves_human_auth_and_invalid_explicit_keys_do_not_fall_back() {
    assert!(credential(None, None).assert_value().is_none());
    for value in ["", "has space", "token\nheader", "token\0suffix"] {
        let error = credential(Some(value), None).assert_error().to_string();
        assert!(error.contains("ZEROSHOT_API_KEY"));
        assert!(!error.contains(value) || value.is_empty());
    }
    assert!(credential(Some("test-api-key"), Some("not-a-uuid")).is_err());
}

#[test]
fn organization_selection_is_a_separate_transport_header() {
    let credential = credential(
        Some("test-api-key"),
        Some("00000000-0000-4000-8000-000000000001"),
    )
    .assert_value()
    .assert_value();
    assert!(!format!("{credential:?}").contains("test-api-key"));
    assert_eq!(credential.api_key, "test-api-key");
    assert_eq!(
        credential.organization.assert_value(),
        "00000000-0000-4000-8000-000000000001"
    );
}

#[cfg(unix)]
#[test]
fn machine_credentials_reject_non_utf8_key_and_organization_values() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let key_error = MachineCredential::from_environment_with(|name| {
        (name == "ZEROSHOT_API_KEY").then(|| OsString::from_vec(vec![0xff]))
    })
    .assert_error()
    .to_string();
    assert!(key_error.contains("ZEROSHOT_API_KEY must be valid UTF-8"));
    let organization_error = MachineCredential::from_environment_with(|name| match name {
        "ZEROSHOT_API_KEY" => Some(OsString::from("test-api-key")),
        "ZEROSHOT_ORGANIZATION" => Some(OsString::from_vec(vec![0xff])),
        _ => None,
    })
    .assert_error()
    .to_string();
    assert!(organization_error.contains("ZEROSHOT_ORGANIZATION must be an organization UUID"));
}

#[test]
fn machine_discovery_rejects_missing_capability_and_authority_changes() {
    let origin = Url::parse("https://example.com").assert_value();
    let mut wire = TargetDiscoveryDocument::direct(
        openengine_cluster_protocol::TargetAuthentication::HostedOauth,
    );
    assert!(validate_discovery(&origin, &wire).is_err());
    wire.extensions.api_key = Some(openengine_cluster_protocol::TargetApiKeyDiscovery {
        kind: API_KEY_AUTHENTICATION_KIND.to_owned(),
        header: "Authorization".to_owned(),
        scheme: "Bearer".to_owned(),
        organization_header: MACHINE_ORGANIZATION_HEADER.to_owned(),
        identity_path: "https://elsewhere.example/machine".to_owned(),
        delegations_path: "/auth/v1/delegations".to_owned(),
    });
    assert!(validate_discovery(&origin, &wire).is_err());
    wire.extensions
        .api_key
        .as_mut()
        .assert_value()
        .identity_path = "/api/v1/machine".to_owned();
    validate_discovery(&origin, &wire).assert_value();
}
