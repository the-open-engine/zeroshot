use openengine_cluster_testkit::assertions::AssertError;

use super::*;

#[test]
fn spawn_detail_names_program_and_escapes_controls_and_delimiters() {
    let error = io::Error::from(io::ErrorKind::NotFound);
    let detail = spawn_error_detail("/missing/tool\n\r\t\u{1b}\u{85}\"\\", &error);

    assert!(detail.starts_with("process spawn failed: kind=NotFound, raw_os_error=none"));
    assert!(detail.ends_with(r#"; program="/missing/tool\n\r\t\u{1b}\u{85}\"\\""#));
    assert!(!detail.chars().any(char::is_control));
}

#[test]
fn spawn_detail_bounds_program_without_splitting_unicode_or_escapes() {
    let error = io::Error::from(io::ErrorKind::NotFound);
    for (program, expected) in [
        ("a".repeat(256), "a".repeat(256)),
        ("a".repeat(257), format!("{}...", "a".repeat(256))),
        ("é".repeat(129), format!("{}...", "é".repeat(128))),
        ("\u{1b}".repeat(60), format!("{}...", r"\u{1b}".repeat(42))),
    ] {
        let detail = spawn_error_detail(&program, &error);
        assert!(
            detail.ends_with(&format!("; program=\"{expected}\"")),
            "{detail}"
        );
        assert!(expected.len() <= MAX_PROGRAM_PREVIEW_BYTES + 3);
        assert!(!detail.chars().any(char::is_control));
    }
}

#[test]
fn io_detail_preserves_structured_cause_and_sanitizes_controls() {
    let error = io::Error::new(io::ErrorKind::PermissionDenied, "denied\nsecret-path");

    let detail = io_error_detail("stdout read failed", &error);

    assert_eq!(
        detail,
        "stdout read failed: kind=PermissionDenied, raw_os_error=none, message=denied secret-path"
    );
}

#[tokio::test]
async fn join_detail_classifies_panics_without_exposing_the_payload() {
    let joined = tokio::spawn(async {
        assert!(std::hint::black_box(false), "sensitive panic payload");
    })
    .await
    .assert_error();

    let detail = task_join_detail("stderr task failed", &joined);

    assert_eq!(detail, "stderr task failed: task panicked");
    assert!(!detail.contains("sensitive"));
}
