use std::io;
use std::path::{Path, PathBuf};
use crate::native_v2_runner::conversations::{ConversationLayout, select_files};

pub(crate) fn layout(home: PathBuf) -> ConversationLayout {
    ConversationLayout {
        harness: "codex",
        storage: home.join("sessions"),
        select,
    }
}
fn select(root: &Path, id: &str) -> io::Result<Vec<PathBuf>> {
    let suffix = format!("-{id}.jsonl");
    select_files(root, |name| {
        name.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(&suffix))
    })
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
