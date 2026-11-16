use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;

use super::super::{FileTargetRegistry, TargetRegistry};
use super::fixtures::{target, temp_root};

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
    let error = registry.get("platform").assert_error().to_string();
    assert!(error.contains(&path.display().to_string()), "{error}");
    assert!(error.contains("I/O failed"), "{error}");
}
