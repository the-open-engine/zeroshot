use super::*;

#[cfg(unix)]
#[tokio::test]
async fn checkpoint_conversation_moves_between_empty_homes_and_workspaces() {
    crate::harnesses::conversation_tests::exercise("codex").await;
}

#[cfg(unix)]
#[tokio::test]
async fn checkpoint_rejects_missing_and_stale_usage_baselines() {
    crate::harnesses::conversation_tests::codex_requires_current_usage().await;
}

#[test]
fn checkpoint_rejects_a_restored_conversation_without_usage() {
    use crate::native_v2_runner::conversations::ConversationMetadata;
    use openengine_cluster_testkit::assertions::AssertValue;
    let root = tempfile::tempdir().assert_value();
    let before = layout(root.path().join("before"));
    std::fs::create_dir_all(&before.storage).assert_value();
    std::fs::write(
        before.storage.join("rollout-checkpoint-id.jsonl"),
        b"first turn",
    )
    .assert_value();
    let metadata = ConversationMetadata {
        harness: "codex".to_owned(),
        id: "checkpoint-id".to_owned(),
        usage: None,
    };
    assert_eq!(
        before.capture(metadata.clone()).err(),
        Some("Codex cumulative usage baseline is unavailable for the checkpoint turn")
    );
    let source = crate::native_v2_runner::conversations::ConversationSource::new(
        metadata,
        before.storage,
        vec![PathBuf::from("rollout-checkpoint-id.jsonl")],
    );
    let after = layout(root.path().join("after"));
    assert_eq!(
        after.restore(&source, None),
        Err("Codex cumulative usage baseline is unavailable for the checkpoint turn")
    );
    assert!(!after.storage.exists());
}

#[test]
fn checkpoint_file_layout_is_portable() {
    crate::native_v2_runner::conversations::layout_tests::roundtrip(
        |home, _| layout(home),
        "2026/11/14/rollout-checkpoint-id.jsonl",
    );
}
