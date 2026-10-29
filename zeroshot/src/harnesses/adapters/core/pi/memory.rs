//! Pi's native tool-result extension for the shared Active Memory broker.

use std::io;
use std::path::Path;

use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_capsule::provider_process::write_provider_file;

pub(crate) fn install(command: &mut ProcessSessionCommand, capability: &Path) -> io::Result<()> {
    let home = capability
        .parent()
        .ok_or_else(|| io::Error::other("private capability has no parent"))?;
    let source = PI_EXTENSION.replace("CAPABILITY_PATH", &serde_json::to_string(capability)?);
    let extension = write_provider_file(home, "active-memory-extension.mjs", source.as_bytes())?;
    command.argv.extend([
        "--extension".to_owned(),
        extension.to_string_lossy().into_owned(),
    ]);
    Ok(())
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
