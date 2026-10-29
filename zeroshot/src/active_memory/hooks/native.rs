//! Generic bounded callback helpers. Native harness installers live beside their adapters.

use std::io;
use std::path::Path;

pub(crate) const MAX_CONTEXT_BYTES: usize = 32 * 1024;

pub(super) fn bounded_context(context: String) -> String {
    if context.len() > MAX_CONTEXT_BYTES || context.contains('\0') {
        String::new()
    } else {
        context
    }
}

pub(crate) fn hook_command(capability: &Path) -> io::Result<String> {
    Ok(format!(
        "{} __active-memory-hook {}",
        shell_quote(&hook_executable()?),
        shell_quote(capability)
    ))
}

fn hook_executable() -> io::Result<std::path::PathBuf> {
    #[cfg(test)]
    {
        test_hook_executable(std::env::var_os("ZEROSHOT_LIVE_CLI_BINARY"))
    }
    #[cfg(not(test))]
    {
        std::env::current_exe()
    }
}

/// Ignored live tests run in libtest, so their native callbacks need the separately built CLI.
#[cfg(test)]
pub(super) fn test_hook_executable(
    executable: Option<std::ffi::OsString>,
) -> io::Result<std::path::PathBuf> {
    let Some(executable) = executable else {
        return std::env::current_exe();
    };
    let executable = std::path::PathBuf::from(executable);
    if !executable.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "live-test hook executable must be absolute",
        ));
    }
    Ok(executable)
}

fn shell_quote(path: &Path) -> String {
    #[cfg(windows)]
    {
        format!("\"{}\"", path.to_string_lossy().replace('"', "\"\""))
    }
    #[cfg(not(windows))]
    {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }
}

pub(crate) fn command_handler(capability: &Path) -> io::Result<serde_json::Value> {
    Ok(
        serde_json::json!({"type":"command", "command":hook_command(capability)?,
        "timeout":1, "additionalContextLimit":MAX_CONTEXT_BYTES}),
    )
}
