use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;
use zeroshot_engine::native_v2_cli::oecp::TargetConnector;

use super::super::*;
use super::fixtures::{FakeAuthority, FakeDialer, MemoryRegistry, target, temp_root};

#[test]
fn registry_refusal_diagnostics_are_invalid_requests() {
    for source in [
        TargetConnectorError::NotFound("unknown".to_owned()),
        TargetConnectorError::RecoveryAuthorizationUnavailable,
        TargetConnectorError::RecoveryAuthorizationMismatch,
        TargetConnectorError::RecoveryAuthorizationInvalid,
    ] {
        let message = format!("target operation failed: {source}");
        let error = cli_target_error(source);
        let diagnostic = serde_json::to_value(error.diagnostic()).assert_value();
        assert_eq!(diagnostic["schema"], "zeroshot.error/v1");
        assert_eq!(diagnostic["kind"], "invalid_request", "{message}");
        assert_eq!(diagnostic["code"], "request.invalid", "{message}");
        assert_eq!(diagnostic["message"], message);
        assert_eq!(diagnostic["details"], json!({}));
    }
}

#[tokio::test]
async fn unknown_target_is_rejected_before_contacting_authority() {
    let authority = FakeAuthority::new("wss://target.example/oecp");
    let connector = NativeV2TargetConnector::new(
        MemoryRegistry::default(),
        authority.clone(),
        FakeDialer::default(),
    );
    let error = connector
        .hosted_run_list(
            "unknown",
            openengine_cluster_protocol::RunListParams::default(),
        )
        .await
        .assert_error();
    let diagnostic = serde_json::to_value(error.diagnostic()).assert_value();
    assert_eq!(diagnostic["code"], "request.invalid");
    assert!(authority.calls().is_empty());
}

#[test]
fn malformed_registry_names_file_and_keeps_parser_or_validation_detail() {
    let root = temp_root();
    let path = root.path("config/targets.json");
    std::fs::create_dir_all(path.parent().assert_value()).assert_value();
    let registry = FileTargetRegistry::new(path.clone());
    let valid = json!({"version":5,"targets":{"prod":target()}});
    for (field, value, detail) in [
        ("label", json!("new-field"), "unknown field `label`"),
        (
            "id",
            json!("not-a-uuid"),
            "invalid stored target record \"prod\"",
        ),
        (
            "name",
            json!("mismatched"),
            "invalid stored target record \"prod\"",
        ),
        (
            "origin",
            json!("https://target.example/path"),
            "invalid stored target record \"prod\"",
        ),
        (
            "access",
            json!({"mode":"hosted","deviceToken":"invalid"}),
            "invalid stored target record \"prod\"",
        ),
    ] {
        let mut state = valid.clone();
        state["targets"]["prod"][field] = value;
        let encoded = serde_json::to_vec(&state).assert_value();
        std::fs::write(&path, &encoded).assert_value();
        // An unrelated target read and an insertion both still reject the entire store.
        for error in [
            registry.get("platform").assert_error(),
            registry.insert(target()).assert_error(),
        ] {
            let message = error.to_string();
            assert!(message.contains(&path.display().to_string()), "{message}");
            assert!(message.contains(detail), "{message}");
            if field == "label" {
                assert!(message.contains("at line 1 column"), "{message}");
            }
        }
        assert_eq!(std::fs::read(&path).assert_value(), encoded);
    }

    for (encoded, detail) in [
        (
            r#"{"version":99,"targets":{}}"#,
            "unsupported target registry version 99",
        ),
        (
            r#"{"version":5,"targets":{},"extra":true}"#,
            "unknown field `extra`",
        ),
        (r#"{"version":5,"targets": "#, "EOF while parsing"),
    ] {
        std::fs::write(&path, encoded).assert_value();
        let message = registry.get("platform").assert_error().to_string();
        assert!(message.contains(&path.display().to_string()), "{message}");
        assert!(message.contains(detail), "{message}");
        assert_eq!(std::fs::read_to_string(&path).assert_value(), encoded);
    }
}

#[test]
fn registry_read_errors_name_the_file() {
    let root = temp_root();
    let path = root.path("config/targets.json");
    std::fs::create_dir_all(path.parent().assert_value()).assert_value();
    let registry = FileTargetRegistry::new(path.clone());
    std::fs::write(&path, vec![b' '; 1024 * 1024 + 1]).assert_value();
    let error = registry.get("platform").assert_error().to_string();
    assert!(error.contains(&path.display().to_string()), "{error}");
    assert!(error.contains("exceeds 1 MiB"), "{error}");

    std::fs::remove_file(&path).assert_value();
    std::fs::create_dir(&path).assert_value();
    let error = registry.get("platform").assert_error();
    let mut cause = std::error::Error::source(&error);
    let mut has_os_error = false;
    while let Some(source) = cause {
        has_os_error |= source
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.raw_os_error().is_some());
        cause = source.source();
    }
    assert!(has_os_error, "original OS error was lost: {error:?}");
    let message = error.to_string();
    assert!(message.contains(&path.display().to_string()), "{message}");
    assert!(message.contains("I/O failed"), "{message}");
}
