//! Native OpenCode tool-result carrier for the host-owned memory broker.

use std::io;
use std::path::Path;

use crate::execution::process::ProcessSessionCommand;

pub(super) fn install(command: &mut ProcessSessionCommand, capability: &Path) -> io::Result<()> {
    let home = capability
        .parent()
        .ok_or_else(|| io::Error::other("memory capability has no private parent"))?;
    let source = PLUGIN
        .replace("CAPABILITY_PATH", &serde_json::to_string(capability)?)
        .replace(
            "MAX_CONTEXT_BYTES",
            &crate::active_memory::MAX_INJECTION_BYTES.to_string(),
        );
    super::plugin::install(command, home, "zeroshot-active-memory", &source)
}

// This plugin imports only native modules; installing memory never installs an npm dependency.
// An absolute timer bounds the whole callback, including peers that keep sending partial frames.
const PLUGIN: &str = r#"import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
const capabilityPath = CAPABILITY_PATH;
const maxContextBytes = MAX_CONTEXT_BYTES;
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
        if (data.length > maxContextBytes + 4) return fail();
        if (data.length < 4) return;
        const length = data.readUInt32BE();
        if (length > maxContextBytes) return fail();
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
export default {
  id: 'zeroshot-active-memory',
  async setup(ctx) {
    fs.mkdirSync(path.dirname(LOADED_MARKER), {recursive: true}); fs.writeFileSync(LOADED_MARKER, '');
    await ctx.tool.hook('execute.after', async (event) => {
      if (event.status !== 'completed' || event.result == null || typeof event.result !== 'object') return;
      const delivery = await retrieve({toolName: event.tool, input: event.input, cwd: ctx.location.directory, sessionId: event.sessionID});
      if (!delivery) return;
      if (Array.isArray(event.result.content)) event.result = {...event.result, content: [...event.result.content, {type:'text', text: delivery.context}]};
      else if (typeof event.result.content === 'string') event.result = {...event.result, content: event.result.content + delivery.context};
      else if (typeof event.result.output === 'string') event.result = {...event.result, output: event.result.output + delivery.context};
      else return;
      delivery.returned();
    });
  }
};
"#;
