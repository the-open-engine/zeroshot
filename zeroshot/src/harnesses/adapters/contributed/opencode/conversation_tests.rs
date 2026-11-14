use super::*;

#[cfg(unix)]
#[tokio::test]
async fn checkpoint_conversation_moves_between_empty_homes_and_workspaces() {
    crate::harnesses::conversation_tests::exercise("opencode").await;
}

#[test]
fn checkpoint_file_layout_is_portable() {
    crate::native_v2_runner::conversations::layout_tests::roundtrip(
        |home, _| layout(&home),
        "opencode.db",
    );
}

#[test]
fn checkpoint_restore_assigns_nested_storage_ancestors_to_provider() {
    crate::native_v2_runner::conversations::layout_tests::nested_storage_ownership(
        layout,
        "opencode.db",
    );
}
