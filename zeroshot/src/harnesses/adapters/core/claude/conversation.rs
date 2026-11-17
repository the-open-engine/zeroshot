use std::io;
use std::path::{Path, PathBuf};
use crate::native_v2_runner::conversations::{ConversationLayout, process_cwd, select_files};

pub(super) fn layout(home: PathBuf, workspace: &Path) -> ConversationLayout {
    // Claude files transcripts by cwd; restore into the successor's project directory.
    ConversationLayout {
        harness: "claude",
        storage: home
            .join("projects")
            .join(project_directory(&process_cwd(workspace))),
        select,
        relocate: None,
    }
}

/// Claude Code's own name for a project directory: one dash for every UTF-16 unit that is not
/// an ASCII letter or digit. Past 200 characters it keeps that prefix and appends its hash of
/// the path, so a longer workspace path is found under no other name.
fn project_directory(cwd: &str) -> String {
    const MAX_LENGTH: usize = 200;
    let mut project = cwd
        .encode_utf16()
        .map(|unit| match u8::try_from(unit) {
            Ok(byte) if byte.is_ascii_alphanumeric() => char::from(byte),
            _ => '-',
        })
        .collect::<String>();
    if project.len() > MAX_LENGTH {
        let hash = cwd.encode_utf16().fold(0_i32, |hash, unit| {
            hash.wrapping_mul(31).wrapping_add(i32::from(unit))
        });
        let mut value = hash.unsigned_abs();
        let mut digits = Vec::new();
        loop {
            digits.push(char::from_digit(value % 36, 36).unwrap_or('0'));
            value /= 36;
            if value == 0 {
                break;
            }
        }
        project.truncate(MAX_LENGTH);
        project.push('-');
        project.extend(digits.iter().rev());
    }
    project
}
fn select(root: &Path, id: &str) -> io::Result<Vec<PathBuf>> {
    let transcript = PathBuf::from(format!("{id}.jsonl"));
    select_files(root, |name| name == transcript || name.starts_with(id))
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
