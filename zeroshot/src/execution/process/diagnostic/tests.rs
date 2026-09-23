use openengine_cluster_testkit::assertions::AssertError;

use super::*;

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
