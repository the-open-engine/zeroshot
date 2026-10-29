//! Native OpenCode tool-result carrier for the host-owned memory broker.

use std::io;
use std::path::Path;

use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_capsule::provider_process::write_provider_file;

pub(super) fn install(command: &mut ProcessSessionCommand, capability: &Path) -> io::Result<()> {
    let home = capability
        .parent()
        .ok_or_else(|| io::Error::other("memory capability has no private parent"))?;
    let source = PLUGIN.replace("CAPABILITY_PATH", &serde_json::to_string(capability)?);
    let plugin = write_provider_file(home, "active-memory-opencode.mjs", source.as_bytes())?;
    super::plugin::append(command, &plugin)
}

// This plugin imports only native modules; installing memory never installs an npm dependency.
// An absolute timer bounds the whole callback, including peers that keep sending partial frames.
const PLUGIN: &str = r#"import fs from 'node:fs';
import net from 'node:net';
const capabilityPath = CAPABILITY_PATH;
function retrieve(event) {
  return new Promise((resolve) => {
    let socket, timer;
    let settled = false;
    const close = () => { clearTimeout(timer); if (socket) socket.destroy(); };
    const fail = () => { close(); if (!settled) { settled = true; resolve(null); } };
    try {
      const cap = JSON.parse(fs.readFileSync(capabilityPath, 'utf8'));
      const split = cap.endpoint.lastIndexOf(':');
      const host = cap.endpoint.slice(0, split);
      if (host !== '127.0.0.1') return fail();
      const port = Number(cap.endpoint.slice(split + 1));
      if (!Number.isInteger(port) || port < 1 || port > 65535) return fail();
      const body = Buffer.from(JSON.stringify({token:cap.token,event}));
      if (body.length > 262144) return fail();
      const header = Buffer.alloc(4); header.writeUInt32BE(body.length);
      let data = Buffer.alloc(0);
      timer = setTimeout(fail, 250);
      socket = net.createConnection({host,port}, () => socket.write(Buffer.concat([header,body])));
      socket.setNoDelay(true);
      socket.on('error', fail); socket.on('end', fail);
      socket.on('data', (chunk) => {
        data = Buffer.concat([data,chunk]);
        if (data.length > 32772) return fail();
        if (data.length < 4) return;
        const length = data.readUInt32BE();
        if (length > 32768) return fail();
        if (data.length < length + 4) return;
        const context = data.subarray(4,length+4).toString('utf8');
        if (!context) return fail();
        if (settled) return;
        settled = true;
        resolve({context,returned:() => {clearTimeout(timer);socket.end(Buffer.from([1]));}});
      });
    } catch { fail(); }
  });
}
export default async ({directory}) => ({
  'tool.execute.after': async (input, output) => {
    const delivery = await retrieve({toolName:input.tool,input:input.args,cwd:directory,sessionId:input.sessionID});
    if (!delivery) return;
    if (Array.isArray(output?.content)) output.content.push({type:'text',text:delivery.context});
    else if (typeof output?.output === 'string') output.output += delivery.context;
    else return;
    delivery.returned();
  }
});
"#;
