use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::native_v2_runner::conversations::{
    ConversationLayout, ConversationRelocation, RelocationCheck, process_cwd, select_files,
};

/// Longest session header read. Pi writes one short JSON line.
const MAX_HEADER_BYTES: u64 = 64 * 1024;

pub(super) fn layout(home: &Path, workspace: &Path) -> ConversationLayout {
    ConversationLayout {
        harness: "pi",
        storage: super::session_id::session_directory(home),
        select,
        relocate: Some(ConversationRelocation {
            workspace: workspace.to_owned(),
            apply: relocate,
        }),
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

/// Pi finds a session ID only among the sessions of its working directory, which every session
/// file names in its first line. A restored file names the predecessor's workspace, so its header
/// line is written again with this one; every later line is copied unchanged. A first line that
/// is no such header is an error: restored as it is, Pi would start an empty session under the
/// same ID while the run believed the conversation continued.
fn relocate(file: &Path, workspace: &Path, check: &mut RelocationCheck<'_>) -> io::Result<()> {
    let mut reader = io::BufReader::new(fs::File::open(file)?);
    let mut header = Vec::new();
    reader
        .by_ref()
        .take(MAX_HEADER_BYTES)
        .read_until(b'\n', &mut header)?;
    let Ok(Value::Object(mut fields)) = serde_json::from_slice::<Value>(&header) else {
        return Err(io::ErrorKind::InvalidData.into());
    };
    let cwd = process_cwd(workspace);
    match fields.get("cwd") {
        Some(Value::String(stored)) if *stored == cwd => return Ok(()),
        Some(Value::String(_)) => {}
        _ => return Err(io::ErrorKind::InvalidData.into()),
    }
    fields.insert("cwd".to_owned(), Value::String(cwd));
    let mut name = file.as_os_str().to_owned();
    name.push(".relocated");
    let relocated = PathBuf::from(name);
    let mut output = crate::execution::platform::private_file(
        &relocated,
        crate::execution::platform::FileAccess::CreateNew,
    )?;
    serde_json::to_writer(&mut output, &fields)?;
    output.write_all(b"\n")?;
    let mut buffer = [0; 64 * 1024];
    loop {
        check()?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
    }
    // Windows replaces a file only once no handle on it is open.
    drop(reader);
    drop(output);
    fs::rename(&relocated, file)
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
