use super::*;

#[cfg(unix)]
#[tokio::test]
async fn checkpoint_conversation_moves_between_empty_homes_and_workspaces() {
    crate::harnesses::conversation_tests::exercise("hermes").await;
}

#[test]
fn checkpoint_file_layout_is_portable() {
    crate::native_v2_runner::conversations::layout_tests::roundtrip(
        |home, _| layout(&home),
        "hermes.sqlite",
    );
}
