//! Claude Code's native tool hook settings.

use std::io;
use std::path::Path;

use crate::active_memory::hooks::command_handler;
use crate::execution::process::ProcessSessionCommand;

pub(crate) fn install(command: &mut ProcessSessionCommand, capability: &Path) -> io::Result<()> {
    let mut settings = serde_json::json!({
        "hooks":{"PreToolUse":[{"hooks":[command_handler(capability)?]}]}
    });
    // A single --settings layer composes native overrides with the required owned hook.
    // Native user/project hooks remain in their own configuration layers.
    if let Some(index) = command
        .argv
        .iter()
        .position(|arg| arg == "--settings" || arg.starts_with("--settings="))
    {
        let inline = command.argv[index]
            .strip_prefix("--settings=")
            .map(str::to_owned)
            .or_else(|| command.argv.get(index + 1).cloned());
        if let Some(mut native) =
            inline.and_then(|inline| serde_json::from_str::<serde_json::Value>(&inline).ok())
        {
            if let Some(object) = native.as_object_mut() {
                object.insert("hooks".to_owned(), settings["hooks"].take());
                let combined = serde_json::to_string(&native)?;
                if command.argv[index] == "--settings" {
                    command.argv[index + 1] = combined;
                } else {
                    command.argv[index] = format!("--settings={combined}");
                }
                return Ok(());
            }
        }
    }
    command
        .argv
        .extend(["--settings".to_owned(), serde_json::to_string(&settings)?]);
    Ok(())
}
