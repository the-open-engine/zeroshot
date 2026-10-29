//! Codex's native hook and exact owned-handler trust configuration.

use std::io;
use std::path::Path;

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::active_memory::hooks::command_handler;
use crate::execution::process::ProcessSessionCommand;

pub(crate) fn install(command: &mut ProcessSessionCommand, capability: &Path) -> io::Result<()> {
    let handler = command_handler(capability)?;
    // Codex collects all configuration layers and retains each user's separate hook trust.
    command.argv.splice(1..1, codex_configuration(&handler)?);
    Ok(())
}

fn codex_configuration(handler: &serde_json::Value) -> io::Result<[String; 4]> {
    // Codex 0.155/0.163 hashes normalized TOML as sorted JSON. Authorize only this
    // invocation's owned handler; a blanket trust bypass would enable unrelated user hooks.
    let mut normalized = handler.clone();
    normalized["async"] = json!(false);
    let mut identity = json!({"event_name":"pre_tool_use","hooks":[normalized]});
    identity.sort_all_objects();
    let trusted_hash = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&identity)?)
    );
    let source = if cfg!(windows) {
        r"C:\<session-flags>\config.toml:pre_tool_use:0:0"
    } else {
        "/<session-flags>/config.toml:pre_tool_use:0:0"
    };
    // Set the map in one override: Codex's dotted CLI-key parser does not preserve
    // quoting around the period in config.toml. User-layer trust remains independent.
    let state = json!({source:{"trusted_hash":trusted_hash}});
    Ok([
        "--config".to_owned(),
        format!(
            "hooks.PreToolUse={}",
            toml_value(&json!([{"hooks":[handler]}]))?
        ),
        "--config".to_owned(),
        format!("hooks.state={}", toml_value(&state)?),
    ])
}

fn toml_value(value: &serde_json::Value) -> io::Result<String> {
    Ok(match value {
        serde_json::Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| {
                    Ok(format!(
                        "{}={}",
                        serde_json::to_string(key)?,
                        toml_value(value)?
                    ))
                })
                .collect::<io::Result<Vec<_>>>()?
                .join(",")
        ),
        serde_json::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(toml_value)
                .collect::<io::Result<Vec<_>>>()?
                .join(",")
        ),
        _ => serde_json::to_string(value)?,
    })
}

#[cfg(test)]
mod tests;
