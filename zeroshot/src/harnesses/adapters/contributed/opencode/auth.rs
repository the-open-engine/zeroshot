//! Owned native plugin that verifies declared provider authority on every model request.

use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_capsule::provider_process::ProviderExecutionFiles;
use crate::native_v2_runner::{DriverInvocation, NodeRunnerError};

use super::{OpenCodeAdapter, command};

pub(super) struct Installation<'a> {
    pub(super) adapter: &'a OpenCodeAdapter,
    pub(super) invocation: &'a DriverInvocation,
    pub(super) files: &'a ProviderExecutionFiles,
    pub(super) command: &'a mut ProcessSessionCommand,
}

pub(super) fn install(installation: Installation<'_>) -> Result<(), NodeRunnerError> {
    let Installation {
        adapter,
        invocation,
        files,
        command,
    } = installation;
    let provider = &adapter.config.provider;
    let endpoint = command::endpoint_fields(provider).iter().find(|field| {
        invocation
            .environment
            .iter()
            .any(|(name, _)| name.as_str() == **field)
    });
    if endpoint.is_some_and(|field| {
        command
            .environment
            .get(*field)
            .is_none_or(|value| value.trim().is_empty())
    }) {
        return Err(NodeRunnerError::DriverDetail(
            "OpenCode declared provider endpoint is empty".to_owned(),
        ));
    }
    let header = if provider == "gateway" {
        command::gateway_protocol(&command.environment)?.1
    } else if provider == "anthropic" {
        "x-api-key"
    } else {
        "Authorization"
    };
    let selection = serde_json::json!({
        "provider": command::native_provider(provider),
        "key": command::credential_field(provider),
        "endpoint": endpoint,
        "header": header,
    });
    let source = HOOK.replace("AUTH_SELECTION", &selection.to_string());
    super::plugin::install(command, files.home(), "zeroshot-provider-auth", &source).map_err(|_| {
        NodeRunnerError::DriverDetail(
            "OpenCode provider authentication hook could not be installed".to_owned(),
        )
    })
}

// The plugin only verifies: the declared key and endpoint reach OpenCode through the inline
// config, and a conflict error names headers, never values. Both the HTTP and the WebSocket
// transport are checked.
const HOOK: &str = r#"import fs from 'node:fs';
import path from 'node:path';
const selection = AUTH_SELECTION;
const authHeader = (name) => /^(authorization|proxy-authorization|cookie)$/i.test(name) || /(^|[-_])(api[-_]?key|auth|token|secret)([-_]|$)/i.test(name);
const within = (url, base) => {
  let request, root;
  try { request = new URL(url); root = new URL(base); } catch { return false; }
  if (request.origin !== root.origin) return false;
  const prefix = root.pathname.replace(/\/+$/, '');
  return request.pathname === prefix || request.pathname.startsWith(prefix + '/');
};
const conflictFile = LOADED_MARKER.replace(/\.loaded$/, '.conflict');
export default {
  id: 'zeroshot-provider-auth',
  async setup(ctx) {
    const key = process.env[selection.key];
    const endpoint = selection.endpoint ? process.env[selection.endpoint] : undefined;
    const canonical = selection.header.toLowerCase();
    const expected = canonical === 'x-api-key' ? key : `Bearer ${key}`;
    function verify(entries, url) {
      const conflicts = [];
      let present = false;
      for (const [name, value] of entries) {
        if (selection.endpoint && name === 'x-provider-api-keys') { conflicts.push(name); continue; }
        if (!authHeader(name)) continue;
        if (name === canonical && key && value === expected) { present = true; continue; }
        conflicts.push(name);
      }
      if (!present) conflicts.push(canonical);
      if (selection.endpoint && !(endpoint && within(url, endpoint))) conflicts.push('endpoint');
      if (!conflicts.length) return;
      const message = `Zeroshot OpenCode declared provider conflict: ${[...new Set(conflicts)].sort().join(', ')}`;
      fs.mkdirSync(path.dirname(conflictFile), {recursive: true}); fs.writeFileSync(conflictFile, message);
      throw new Error(message);
    }
    await ctx.session.hook('http.request', async (event) => {
      if (event.model?.providerID !== selection.provider) return;
      verify([...event.request.headers], event.request.url);
    });
    await ctx.session.hook('experimental.ws.handshake', async (event) => {
      if (event.model?.providerID !== selection.provider) return;
      verify(Object.entries(event.headers ?? {}).map(([name, value]) => [name.toLowerCase(), String(value)]), String(event.url).replace(/^ws(s?):/i, 'http$1:'));
    });
    fs.mkdirSync(path.dirname(LOADED_MARKER), {recursive: true}); fs.writeFileSync(LOADED_MARKER, '');
  }
}
"#;
