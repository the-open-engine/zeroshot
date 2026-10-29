//! Append an owned file plugin without replacing native configuration or plugin discovery.

use std::io;
use std::path::Path;

use serde_json::{Value, json};

use crate::execution::process::ProcessSessionCommand;

pub(super) fn append(command: &mut ProcessSessionCommand, path: &Path) -> io::Result<()> {
    let plugin = url::Url::from_file_path(path)
        .map_err(|_| io::Error::other("OpenCode plugin path is invalid"))?;
    let mut config: Value = serde_json::from_str(
        command
            .environment
            .get("OPENCODE_CONFIG_CONTENT")
            .map(String::as_str)
            .unwrap_or("{}"),
    )?;
    let plugins = config
        .as_object_mut()
        .ok_or_else(|| io::Error::other("inline config is not an object"))?
        .entry("plugin")
        .or_insert_with(|| json!([]));
    plugins
        .as_array_mut()
        .ok_or_else(|| io::Error::other("plugins must be an array"))?
        .push(json!(plugin.as_str()));
    command.environment.insert(
        "OPENCODE_CONFIG_CONTENT".to_owned(),
        serde_json::to_string(&config)?,
    );
    Ok(())
}
