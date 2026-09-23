use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::CodexSchemaFile;
use crate::native_v2_candidate::test_support::TestDirectory;

#[test]
fn schema_file_is_readable_during_the_turn_and_removed_afterward() {
    let runtime = TestDirectory::new("codex-schema-file");
    let path = {
        let schema =
            CodexSchemaFile::create(runtime.path(), &json!({"type":"null"})).assert_value();
        let path = schema.path().to_owned();
        assert_eq!(std::fs::read(&path).assert_value(), br#"{"type":"null"}"#);
        path
    };
    assert!(!path.exists());
}

#[test]
fn schema_file_creation_preserves_io_detail() {
    let runtime = TestDirectory::new("codex-schema-file-error");
    let missing_home = runtime.child("missing/home");
    let error = CodexSchemaFile::create(&missing_home, &json!({"type":"null"}))
        .err()
        .assert_value();

    assert!(error.to_string().contains("could not be created"));
    let io_error = match error {
        super::CodexSchemaFileError::Write(error) => Some(error),
        super::CodexSchemaFileError::Serialize(_) => None,
    }
    .assert_value();
    assert_eq!(io_error.kind(), std::io::ErrorKind::NotFound);
}
