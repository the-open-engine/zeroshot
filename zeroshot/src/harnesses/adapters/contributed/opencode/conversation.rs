use std::io;
use std::path::{Path, PathBuf};
use crate::native_v2_runner::conversations::{ConversationLayout, select_files};

pub(super) fn layout(home: &Path) -> ConversationLayout {
    ConversationLayout {
        harness: "opencode",
        storage: home.join(".local/share/opencode"),
        select,
    }
}
fn select(root: &Path, _id: &str) -> io::Result<Vec<PathBuf>> {
    // Only contained homes have an exclusively owned database. Local shared databases are
    // intentionally unsupported: copying one would capture other conversations.
    select_files(root, |name| {
        matches!(
            name.to_str(),
            Some("opencode.db" | "opencode.db-wal" | "opencode.db-shm")
        )
    })
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
