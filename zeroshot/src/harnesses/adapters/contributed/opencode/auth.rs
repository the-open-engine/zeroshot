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
// config, and a conflict error names headers, never values.
const HOOK: &str = r#"import fs from 'node:fs';
import path from 'node:path';
const selection = AUTH_SELECTION;
const authHeader = (name) => /^(authorization|proxy-authorization|cookie)$/i.test(name) || /(^|[-_])(api[-_]?key|auth|token|secret)([-_]|$)/i.test(name);
const within = (url, base) => {
  const root = base.replace(/\/+$/, '');
  return url === root || url.startsWith(root + '/') || url.startsWith(root + '?');
};
export default {
  id: 'zeroshot-provider-auth',
  async setup(ctx) {
    const key = process.env[selection.key];
    const endpoint = selection.endpoint ? process.env[selection.endpoint] : undefined;
    const canonical = selection.header.toLowerCase();
    const expected = canonical === 'x-api-key' ? key : `Bearer ${key}`;
    fs.mkdirSync(path.dirname(LOADED_MARKER), {recursive: true}); fs.writeFileSync(LOADED_MARKER, '');
    await ctx.session.hook('http.request', async (event) => {
      if (event.model?.providerID !== selection.provider) return;
      const conflicts = [];
      let present = false;
      event.request.headers.forEach((value, name) => {
        if (selection.endpoint && name === 'x-provider-api-keys') { conflicts.push(name); return; }
        if (!authHeader(name)) return;
        if (name === canonical && key && value === expected) { present = true; return; }
        conflicts.push(name);
      });
      if (!present) conflicts.push(canonical);
      if (selection.endpoint && !(endpoint && within(event.request.url, endpoint))) conflicts.push('endpoint');
      if (conflicts.length) throw new Error(`Zeroshot OpenCode declared provider conflict: ${[...new Set(conflicts)].sort().join(', ')}`);
    });
  }
}
"#;
