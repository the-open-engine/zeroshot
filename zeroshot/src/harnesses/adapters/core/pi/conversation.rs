use std::io;
use std::path::{Path, PathBuf};
use crate::native_v2_runner::conversations::{ConversationLayout, select_files};

pub(super) fn layout(home: &Path) -> ConversationLayout {
    ConversationLayout {
        harness: "pi",
        storage: super::session_id::session_directory(home),
        select,
    }
}
fn select(root: &Path, id: &str) -> io::Result<Vec<PathBuf>> {
    let suffix = format!("_{id}.jsonl");
    let exact = format!("{id}.jsonl");
    select_files(root, |name| {
        name.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == exact || name.ends_with(&suffix))
    })
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
