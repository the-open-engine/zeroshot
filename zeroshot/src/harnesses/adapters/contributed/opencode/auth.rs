//! Native hooks apply declared provider authority after native configuration has merged.

use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_capsule::provider_process::ProviderExecutionFiles;
use crate::native_v2_capsule::provider_process::write_provider_file;
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
    if command
        .environment
        .get("OPENCODE_PURE")
        .is_some_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "true" | "1" | "yes" | "on"
            )
        })
        || adapter
            .config
            .prefix_arguments
            .iter()
            .any(|value| matches!(value.as_str(), "--pure" | "--pure=true" | "--pure=1"))
    {
        return Err(NodeRunnerError::DriverDetail("OpenCode pure mode disables the native hook required by declared provider authentication".to_owned()));
    }
    let provider = &adapter.config.provider;
    let mut auth = super::login::without_provider(
        adapter.local_environment.get("OPENCODE_AUTH_CONTENT"),
        command,
        command::native_provider(provider),
    )?;
    auth.select(provider, &command.environment)?;
    command
        .environment
        .insert("OPENCODE_AUTH_CONTENT".to_owned(), auth.masked);
    let inline = auth.inline.is_some();
    if let Some(source) = auth.inline {
        command
            .environment
            .insert(super::login::SOURCE_ENV.to_owned(), source);
    } else {
        command.environment.remove(super::login::SOURCE_ENV);
    }
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
    let mut selection = serde_json::json!({"provider":command::native_provider(provider),"key":command::credential_field(provider),"endpoint":endpoint,"inline":inline,"header":if provider == "anthropic" {"x-api-key"} else {"Authorization"}});
    extend_selection(&mut selection, provider, invocation, command)?;
    let source = HOOK.replace("AUTH_SELECTION", &selection.to_string());
    let path = write_provider_file(
        files.home(),
        "provider-auth-opencode.mjs",
        source.as_bytes(),
    )
    .map_err(|_| {
        NodeRunnerError::DriverDetail(
            "OpenCode provider authentication hook could not be written".to_owned(),
        )
    })?;
    super::plugin::append(command, &path).map_err(|_| {
        NodeRunnerError::DriverDetail(
            "OpenCode provider authentication hook could not be installed".to_owned(),
        )
    })
}

fn extend_selection(
    selection: &mut serde_json::Value,
    provider: &str,
    invocation: &DriverInvocation,
    command: &ProcessSessionCommand,
) -> Result<(), NodeRunnerError> {
    if provider == "bedrock" {
        selection["region"] = serde_json::json!(command.environment.get("AWS_REGION"));
    }
    if provider == "gateway" {
        let (npm, header) = command::gateway_protocol(&command.environment)?;
        let crate::native_v2_contract::NodeRuntimeBinding::Agent { model, .. } =
            &invocation.node.binding
        else {
            return Err(NodeRunnerError::InvalidRole);
        };
        selection["npm"] = serde_json::json!(npm);
        selection["header"] = serde_json::json!(header);
        selection["model"] = serde_json::json!(model.as_str());
    }
    Ok(())
}

// config() runs after native file/inline/managed configuration has merged. Native plugins keep
// their order and all unrelated settings. provider.models handles catalog models before authored
// models merge. chat.headers observes actual SDK inputs and propagates concrete conflict errors
// before a model request; config() errors alone are swallowed by the native plugin loader.
const HOOK: &str = r#"import {AsyncLocalStorage} from 'node:async_hooks';
const selection = AUTH_SELECTION;
const authentication = ['apiKey','authToken','authorization','token','accessKeyId','secretAccessKey','sessionToken'];
const authHeader = (name) => /^(authorization|proxy-authorization|cookie|set-cookie)$/i.test(name) || /(^|[-_])(api[-_]?key|auth|token|secret)([-_]|$)/i.test(name);
const object = (value) => value && typeof value === 'object' && !Array.isArray(value);
const headers = (value) => Object.fromEntries(Object.entries(value ?? {}).filter(([name]) => !selection.endpoint && !authHeader(name)));
function cleanOptions(options) {
  if (!object(options)) return;
  for (const name of authentication) delete options[name];
  if (selection.endpoint) delete options.api_keys;
  if (options.headers) options.headers = headers(options.headers);
}
function cleanModels(models) {
  for (const model of Object.values(models ?? {})) {
    if (!object(model)) continue;
    if (model.headers) model.headers = headers(model.headers);
  }
  return models;
}
function authFields(options, prefix, key) {
  if (!object(options)) return [];
  const fields = authentication.filter((name) => Object.hasOwn(options,name)).map((name) => `${prefix}.${name}`);
  if (selection.endpoint && Object.hasOwn(options,'api_keys')) fields.push(`${prefix}.api_keys`);
  if (headerConflict(options.headers,key)) fields.push(`${prefix}.headers`);
  if (selection.endpoint && Object.hasOwn(options,'baseURL')) fields.push(`${prefix}.baseURL`);
  return fields;
}
function headerConflict(headers, key, normalize = false) {
  const canonical = selection.header;
  const expected = canonical === 'x-api-key' ? key : `Bearer ${key}`;
  let conflict = false;
  for (const [name,value] of Object.entries(headers ?? {})) {
    if (selection.endpoint && name.toLowerCase() === 'x-provider-api-keys') { conflict = true; continue; }
    if (!authHeader(name)) continue;
    if (name.toLowerCase() !== canonical.toLowerCase() || value !== expected) { conflict = true; continue; }
    if (name !== canonical) {
      if (!normalize) { conflict = true; continue; }
      delete headers[name]; headers[canonical] = value;
    }
  }
  return conflict;
}
function updated(auth, id, entry) {
  const next = {...auth}, normalized = id.replace(/\/+$/,'');
  if (normalized !== id) delete next[id];
  delete next[normalized + '/'];
  return {...next,[normalized]:entry};
}
function preserveNativeWrites(client) {
  let inline = selection.inline ? process.env.ZEROSHOT_OPENCODE_AUTH_SOURCE : undefined;
  delete process.env.ZEROSHOT_OPENCODE_AUTH_SOURCE;
  if (!client?.auth?.set) return;
  let masked = JSON.parse(process.env.OPENCODE_AUTH_CONTENT), queue = Promise.resolve();
  const selected = masked[selection.provider], authority = new AsyncLocalStorage(), environment = process.env;
  // Native Auth.set reads Auth.all in its own async scope. Never unmask another model request.
  process.env = new Proxy(environment, {
    get(target,name,receiver) {
      if (name === 'OPENCODE_AUTH_CONTENT' && authority.getStore()) return authority.getStore().source;
      return Reflect.get(target,name,receiver);
    },
    set(target,name,value) { target[name] = value; return true; }
  });
  const set = client.auth.set.bind(client.auth);
  client.auth.set = (...args) => {
    const write = queue.then(async () => {
      const result = await authority.run({source:inline}, () => set(...args)), request = args[0];
      if (result?.data === true && typeof request?.path?.id === 'string') {
        masked = updated(masked,request.path.id,request.body);
        delete masked[selection.provider];
        if (selected !== undefined) masked[selection.provider] = selected;
        if (inline !== undefined) inline = JSON.stringify(updated(JSON.parse(inline),request.path.id,request.body));
      }
      process.env.OPENCODE_AUTH_CONTENT = JSON.stringify(masked);
      return result;
    });
    queue = write.then(() => undefined, () => undefined);
    return write;
  };
}
export default async ({client}) => {
  preserveNativeWrites(client);
  const key = process.env[selection.key];
  const endpoint = selection.endpoint ? process.env[selection.endpoint] : undefined;
  let configured = false;
  return {
    config: async (config) => {
      const providers = config.provider ??= {};
      const provider = providers[selection.provider] ??= {};
      const options = provider.options ??= {};
      cleanOptions(options);
      options.apiKey = key;
      if (selection.endpoint) {
        options.baseURL = endpoint;
        if (selection.provider === 'amazon-bedrock') options.endpoint = endpoint;
      }
      if (selection.region) options.region = selection.region;
      if (selection.npm) {
        provider.npm = selection.npm;
        const model = (provider.models ??= {})[selection.model] ??= {name:selection.model};
        (model.provider ??= {}).npm = selection.npm;
      }
      cleanModels(provider.models);
      configured = true;
    },
    provider: {id:selection.provider, models:async (provider) => cleanModels(provider.models)},
    'chat.headers': async (input, output) => {
      if (input.model.providerID !== selection.provider) return;
      let headers = output.headers;
      Object.defineProperty(output,'headers',{
        enumerable:true,configurable:true,
        set(value) { headers = value; },
        get() {
          const options = input.provider.options, fields = [];
          if (!configured || !key || options?.apiKey !== key) fields.push('provider.options.apiKey');
          if (selection.endpoint && options?.baseURL !== endpoint) fields.push('provider.options.baseURL');
          if (selection.endpoint && selection.provider === 'amazon-bedrock' && options?.endpoint !== endpoint) fields.push('provider.options.endpoint');
          if (selection.region && options?.region !== selection.region) fields.push('provider.options.region');
          if (selection.npm && input.model.api?.npm !== selection.npm) fields.push('model.api.npm');
          fields.push(...authFields({...options,apiKey:undefined},'provider.options',key).filter((name) => name !== 'provider.options.apiKey' && name !== 'provider.options.baseURL'));
          if (headerConflict(input.model.headers,key)) fields.push('model.headers');
          if (headerConflict(headers,key,true)) fields.push('chat.headers');
          if (fields.length) throw new Error(`Zeroshot OpenCode declared provider conflict: ${[...new Set(fields)].join(', ')}`);
          return headers;
        }
      });
      // The SDK has already captured provider/model settings; retain this initial proof too.
      void output.headers;
    }
  };
};
"#;
