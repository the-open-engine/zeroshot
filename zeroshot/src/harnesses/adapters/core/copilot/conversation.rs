use std::io;
use std::path::{Path, PathBuf};
use crate::native_v2_runner::conversations::{ConversationLayout, select_files};

pub(super) fn layout(home: PathBuf) -> ConversationLayout {
    ConversationLayout {
        harness: "copilot",
        storage: home.join("session-state"),
        select,
    }
}
fn select(root: &Path, id: &str) -> io::Result<Vec<PathBuf>> {
    select_files(root, |name| name.starts_with(id))
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
