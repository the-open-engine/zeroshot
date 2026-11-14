use super::*;

#[cfg(unix)]
#[tokio::test]
async fn checkpoint_conversation_moves_between_empty_homes_and_workspaces() {
    crate::harnesses::conversation_tests::exercise("pi").await;
}

#[test]
fn checkpoint_file_layout_is_portable() {
    crate::native_v2_runner::conversations::layout_tests::roundtrip(
        |home, _| layout(&home),
        "2026-11-14_checkpoint-id.jsonl",
    );
}
