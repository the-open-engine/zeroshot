use std::io;
use std::path::Path;

use serde_json::json;
use sha2::{Digest, Sha256};

use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_capsule::provider_process::write_provider_file;

use super::broker::Capability;

pub(super) const MAX_CONTEXT_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy)]
pub(crate) enum Harness {
    Codex,
    Claude,
    Pi,
}

pub(super) fn bounded_context(context: String) -> String {
    if context.len() > MAX_CONTEXT_BYTES || context.contains('\0') {
        String::new()
    } else {
        context
    }
}

pub(super) fn configure(
    harness: Harness,
    command: &mut ProcessSessionCommand,
    home: &Path,
    capability: &Capability,
) -> io::Result<()> {
    let bytes = serde_json::to_vec(capability)?;
    let path = write_provider_file(home, "active-memory-capability.json", &bytes)?;
    match harness {
        Harness::Codex | Harness::Claude => configure_command(harness, command, &path)?,
        Harness::Pi => {
            let source = PI_EXTENSION.replace("CAPABILITY_PATH", &serde_json::to_string(&path)?);
            let extension =
                write_provider_file(home, "active-memory-extension.mjs", source.as_bytes())?;
            command.argv.extend([
                "--extension".to_owned(),
                extension.to_string_lossy().into_owned(),
            ]);
        }
    }
    Ok(())
}

fn configure_command(
    harness: Harness,
    command: &mut ProcessSessionCommand,
    path: &Path,
) -> io::Result<()> {
    let executable = std::env::current_exe()?;
    let hook = format!(
        "{} __active-memory-hook {}",
        shell_quote(&executable),
        shell_quote(path)
    );
    let handler = json!({"type":"command","command":hook,"timeout":1,"additionalContextLimit":MAX_CONTEXT_BYTES});
    if matches!(harness, Harness::Codex) {
        // Codex collects hooks from all config layers, preserving native definitions.
        command.argv.splice(1..1, codex_configuration(&handler)?);
    } else {
        let settings = json!({"hooks":{"PreToolUse":[{"hooks":[handler]}]}});
        command
            .argv
            .extend(["--settings".to_owned(), serde_json::to_string(&settings)?]);
    }
    Ok(())
}

pub(super) fn codex_configuration(handler: &serde_json::Value) -> io::Result<[String; 4]> {
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

// Explicit extension paths are honored even with --no-extensions. No npm imports or child
// process are needed. Errors leave the original result unchanged and cannot block the tool.
const PI_EXTENSION: &str = r#"import fs from 'node:fs';
import net from 'node:net';
const capabilityPath = CAPABILITY_PATH;
function retrieve(event) {
  return new Promise((resolve) => {
    let socket;
    let settled = false;
    const fail = () => { if (socket) socket.destroy(); if (!settled) { settled = true; resolve(null); } };
    const finish = (context) => {
      if (!context) return fail();
      if (settled) return;
      settled = true;
      resolve({context,returned:() => socket.end(Buffer.from([1]))});
    };
    try {
      const cap = JSON.parse(fs.readFileSync(capabilityPath, 'utf8'));
      const split = cap.endpoint.lastIndexOf(':');
      const host = cap.endpoint.slice(0, split);
      if (host !== '127.0.0.1') return fail();
      const body = Buffer.from(JSON.stringify({token:cap.token,event}));
      if (body.length > 262144) return fail();
      const header = Buffer.alloc(4); header.writeUInt32BE(body.length);
      let data = Buffer.alloc(0);
      socket = net.createConnection({host,port:Number(cap.endpoint.slice(split+1))}, () => socket.write(Buffer.concat([header,body])));
      socket.setNoDelay(true); socket.setTimeout(250, fail);
      socket.on('error', fail); socket.on('end', fail);
      socket.on('data', (chunk) => {
        data = Buffer.concat([data,chunk]);
        if (data.length > 32772) return fail();
        if (data.length >= 4) {
          const length = data.readUInt32BE();
          if (length > 32768) return fail();
          if (data.length >= length+4) finish(data.subarray(4,length+4).toString('utf8'));
        }
      });
    } catch { fail(); }
  });
}
export default function(pi) {
  pi.on('tool_result', async (event,ctx) => {
    const delivery = await retrieve({toolName:event.toolName,input:event.input,cwd:ctx.cwd});
    if (!delivery) return;
    const result = {content:[...event.content,{type:'text',text:delivery.context}],
      ...(event.structuredContent === undefined ? {} : {structuredContent:event.structuredContent}),
      ...(event.details === undefined ? {} : {details:event.details}),
      ...(event.usage === undefined ? {} : {usage:event.usage}),isError:event.isError};
    delivery.returned();
    return result;
  });
}
"#;
