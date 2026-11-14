use std::io;
use std::path::{Path, PathBuf};
use crate::native_v2_runner::conversations::{ConversationLayout, select_files};

pub(super) fn layout(home: PathBuf, workspace: &Path) -> ConversationLayout {
    // Claude files transcripts by cwd; restore into the successor's project directory.
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_owned());
    let cwd = workspace.to_string_lossy();
    // Windows canonicalization adds a filesystem-only prefix that process.cwd() omits.
    #[cfg(windows)]
    let cwd = if let Some(unc) = cwd.strip_prefix(r"\\?\UNC\") {
        std::borrow::Cow::Owned(format!(r"\\{unc}"))
    } else {
        std::borrow::Cow::Borrowed(cwd.strip_prefix(r"\\?\").unwrap_or(&cwd))
    };
    let project = cwd
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    ConversationLayout {
        harness: "claude",
        storage: home.join("projects").join(project),
        select,
    }
}
fn select(root: &Path, id: &str) -> io::Result<Vec<PathBuf>> {
    let transcript = PathBuf::from(format!("{id}.jsonl"));
    select_files(root, |name| name == transcript || name.starts_with(id))
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
