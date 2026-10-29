//! Claude Code's native tool hook settings.

use std::io;
use std::path::Path;

use crate::active_memory::hooks::command_handler;
use crate::execution::process::ProcessSessionCommand;

pub(crate) fn install(command: &mut ProcessSessionCommand, capability: &Path) -> io::Result<()> {
    let settings = serde_json::json!({
        "hooks":{"PreToolUse":[{"hooks":[command_handler(capability)?]}]}
    });
    command
        .argv
        .extend(["--settings".to_owned(), serde_json::to_string(&settings)?]);
    Ok(())
}
