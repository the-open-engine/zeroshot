use std::io;
use std::path::{Path, PathBuf};
use crate::native_v2_runner::conversations::{ConversationLayout};

pub(super) fn layout(home: &Path) -> ConversationLayout {
    ConversationLayout {
        harness: "hermes",
        storage: home.to_owned(),
        select,
    }
}
fn select(root: &Path, _id: &str) -> io::Result<Vec<PathBuf>> {
    // This database belongs exclusively to this private node session.
    let mut files = Vec::new();
    for name in ["hermes.sqlite", "hermes.sqlite-wal", "hermes.sqlite-shm"] {
        if root.join(name).try_exists()? {
            files.push(PathBuf::from(name));
        }
    }
    Ok(files)
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
