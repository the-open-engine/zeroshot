use super::*;
use crate::execution::process::{HostedProcessScope, ProcessSessionCommand};
use crate::native_v2_candidate::test_support::TestDirectory;

fn hook_fixture(
    directory: &TestDirectory,
    endpoint: bool,
    native_auth: Option<&str>,
) -> (Arc<ProviderExecutionFiles>, ProcessSessionCommand) {
    let mut adapter = super::process::adapter(directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "openai".to_owned();
    if let Some(source) = native_auth {
        adapter
            .local_environment
            .insert("OPENCODE_AUTH_CONTENT".to_owned(), source.to_owned());
    }
    adapter.local_environment.insert("OPENCODE_CONFIG_CONTENT".to_owned(), json!({
        "plugin":["file:///ordinary-native-plugin.mjs"],
        "mcp":{"native":{"type":"remote","url":"https://mcp.invalid","headers":{"Authorization":"native-mcp-key"}}}
    }).to_string());
    let files = Arc::new(
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value(),
    );
    let mut fields = vec![("OPENAI_API_KEY", "declared-key")];
    if endpoint {
        fields.push(("OPENAI_BASE_URL", "https://declared.invalid/v1"));
    }
    let invocation = super::process::invocation(&fields);
    let command = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &invocation,
        files: &files,
        session_id: None,
    })
    .assert_value();
    (files, command)
}

fn configuration() -> Value {
    let mut models = serde_json::Map::new();
    for id in ["token", "models", "variants"] {
        models.insert(id.to_owned(), json!({"headers":{"Authorization":"native-model-key","X-Trace-ID":"model-trace"},
            "options":{"apiKey":"native-model-key","headers":{"x-api-key":"native-model-key"},"responseFormat":{"schema":{"properties":{"token":{"type":"string"}}}}},
            "variants":{"models":{"authToken":"native-variant-key","reasoningEffort":"high"}}}));
    }
    json!({"permission":{"bash":"deny"},"plugin":["ordinary-native-plugin"],
        "mcp":{"native":{"headers":{"Authorization":"native-mcp-key"}}},
        "provider":{"openai":{"options":{"apiKey":"native-key","authToken":"native-token","api_keys":{"openai":"upstream-private-key"},"baseURL":"https://native.invalid/v2",
            "headers":{"Authorization":"Bearer native-key","HTTP-Referer":"https://native.invalid","X-Trace-ID":"provider-trace"}},"models":models},
            "anthropic":{"options":{"apiKey":"other-provider-key"}}}})
}

fn run_hook(source: &str, endpoint: bool) -> Value {
    run_hook_with_auth(source, endpoint, None)
}

fn run_hook_with_auth(source: &str, endpoint: bool, native_auth: Option<&str>) -> Value {
    let directory = TestDirectory::new("opencode-native-provider-hook");
    let (_files, command) = hook_fixture(&directory, endpoint, native_auth);
    let inline: Value =
        serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    assert_eq!(inline["plugin"][0], "file:///ordinary-native-plugin.mjs");
    assert_eq!(
        inline["mcp"]["native"]["headers"]["Authorization"],
        "native-mcp-key"
    );
    let plugin = inline["plugin"]
        .as_array()
        .assert_value()
        .last()
        .assert_value()
        .as_str()
        .assert_value();
    let input = directory.write("native-config.json", &configuration().to_string());
    let output = node_command()
        .args(["--input-type=module", "-e", source, plugin])
        .arg(input)
        .envs(&command.environment)
        .env("OPENAI_API_KEY", "declared-key")
        .env("OPENAI_BASE_URL", "https://declared.invalid/v1")
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).assert_value()
}

#[test]
fn native_auth_writes_are_serialized_restore_masking_and_preserve_sdk_results() {
    let source = r#"const {default:create}=await import(process.argv[1]);
let active=0,maximum=0;const seen=[];
const remove=()=>{throw new Error('MCP auth method must stay untouched')};
const auth={remove,async set(request,options){
  if(this!==auth || options.custom!==true)throw new Error('native SDK invocation changed');
  seen.push(process.env.OPENCODE_AUTH_CONTENT===undefined?null:JSON.parse(process.env.OPENCODE_AUTH_CONTENT));
  active++;maximum=Math.max(maximum,active);await new Promise(resolve=>setTimeout(resolve,10));active--;
  if(request.path.id==='rejected')throw new Error('native rejection');
  if(request.path.id==='native-error')return {data:false,error:'native failure'};
  return {data:true,provider:request.path.id};
}};
await create({client:{auth}});
const requests=[
  {path:{id:'other///'},body:{type:'oauth',refresh:'new-refresh',access:'new-access',expires:10}},
  {path:{id:'rejected'},body:{type:'api',key:'rejected-key'}},
  {path:{id:'native-error'},body:{type:'api',key:'failed-key'}},
  {path:{id:'openai/'},body:{type:'api',key:'updated-native-login'}},
  {path:{id:'last'},body:{type:'api',key:'last-key'}}
];
const outside=[];const sample=setInterval(()=>outside.push(JSON.parse(process.env.OPENCODE_AUTH_CONTENT)),1);
const results=await Promise.allSettled(requests.map(request=>auth.set(request,{custom:true})));
clearInterval(sample);if(!outside.length||outside.some(auth=>auth.openai))throw new Error('selected native login exposed during a pending write');
console.log(JSON.stringify({maximum,seen,masked:JSON.parse(process.env.OPENCODE_AUTH_CONTENT),
  sourceRetained:process.env.ZEROSHOT_OPENCODE_AUTH_SOURCE!==undefined,mcpUntouched:auth.remove===remove,
  results:results.map(result=>result.status==='fulfilled'?result.value:{error:result.reason.message})}));"#;
    let inline = json!({"openai":{"type":"api","key":"wrong-native-login"},"other/":{"type":"api","key":"old-other-login"}}).to_string();
    for native in [None, Some(inline.as_str())] {
        let returned = run_hook_with_auth(source, false, native);
        assert_eq!(returned["maximum"], 1);
        assert_eq!(returned["sourceRetained"], false);
        assert_eq!(returned["mcpUntouched"], true);
        assert_eq!(returned["results"][0]["provider"], "other///");
        assert_eq!(returned["results"][1]["error"], "native rejection");
        assert_eq!(returned["results"][2]["data"], false);
        assert_eq!(returned["results"][4]["provider"], "last");
        let masked = &returned["masked"];
        for absent in [
            "openai",
            "openai/",
            "other/",
            "other///",
            "rejected",
            "native-error",
        ] {
            assert!(masked.get(absent).is_none());
        }
        assert_eq!(masked["other"]["refresh"], "new-refresh");
        assert_eq!(masked["last"]["key"], "last-key");
        if native.is_some() {
            assert_eq!(returned["seen"][0]["openai"]["key"], "wrong-native-login");
            assert_eq!(returned["seen"][1]["other"]["access"], "new-access");
            assert_eq!(returned["seen"][4]["openai"]["key"], "updated-native-login");
        } else {
            assert!(
                returned["seen"]
                    .as_array()
                    .assert_value()
                    .iter()
                    .all(Value::is_null)
            );
        }
    }
}

const PREPARE: &str = r#"import fs from 'node:fs';
const {default:create}=await import(process.argv[1]);
const original=JSON.parse(fs.readFileSync(process.argv[2],'utf8'));
const config=structuredClone(original);
const hooks=await create({});
await hooks.config(config);
const catalog=await hooks.provider.models({models:structuredClone(original.provider.openai.models)});
const input={provider:config.provider.openai,model:{providerID:'openai',...config.provider.openai.models.token}};
"#;

#[test]
fn declared_endpoint_hook_preserves_native_features_and_opaque_model_identifiers() {
    let source = format!(
        "{PREPARE}\nconst output={{headers:{{}}}}; await hooks['chat.headers'](input,output); void output.headers; console.log(JSON.stringify({{config,catalog}}));"
    );
    let returned = run_hook(&source, true);
    let config = &returned["config"];
    assert!(
        config["provider"]["openai"]["options"]
            .get("api_keys")
            .is_none()
    );
    assert_eq!(config["permission"]["bash"], "deny");
    assert_eq!(config["plugin"], json!(["ordinary-native-plugin"]));
    assert_eq!(
        config["mcp"]["native"]["headers"]["Authorization"],
        "native-mcp-key"
    );
    assert_eq!(
        config["provider"]["anthropic"]["options"]["apiKey"],
        "other-provider-key"
    );
    assert_eq!(
        config["provider"]["openai"]["options"]["apiKey"],
        "declared-key"
    );
    assert_eq!(
        config["provider"]["openai"]["options"]["baseURL"],
        "https://declared.invalid/v1"
    );
    for models in [
        &config["provider"]["openai"]["models"],
        &returned["catalog"],
    ] {
        for id in ["token", "models", "variants"] {
            assert_eq!(models[id]["headers"], json!({}));
            assert_eq!(models[id]["options"]["apiKey"], "native-model-key");
            assert_eq!(
                models[id]["options"]["responseFormat"]["schema"]["properties"]["token"]["type"],
                "string"
            );
            assert_eq!(models[id]["variants"]["models"]["reasoningEffort"], "high");
            assert_eq!(
                models[id]["variants"]["models"]["authToken"],
                "native-variant-key"
            );
        }
    }
}

#[test]
fn key_only_hook_keeps_native_transport_and_non_authentication_headers() {
    let source = format!(
        "{PREPARE}\nconfig.provider.openai.options.baseURL='https://plugin-native.invalid/v3'; const output={{headers:{{'X-Provider-API-Keys':'native-routing'}}}}; await hooks['chat.headers'](input,output); void output.headers; console.log(JSON.stringify({{...config.provider.openai,requestHeaders:output.headers}}));"
    );
    let returned = run_hook(&source, false);
    assert_eq!(
        returned["options"]["api_keys"]["openai"],
        "upstream-private-key"
    );
    assert_eq!(
        returned["options"]["baseURL"],
        "https://plugin-native.invalid/v3"
    );
    assert_eq!(
        returned["options"]["headers"]["HTTP-Referer"],
        "https://native.invalid"
    );
    assert_eq!(
        returned["options"]["headers"]["X-Trace-ID"],
        "provider-trace"
    );
    assert!(
        returned["options"]["headers"]
            .get("Authorization")
            .is_none()
    );
    assert_eq!(
        returned["models"]["token"]["headers"]["X-Trace-ID"],
        "model-trace"
    );
}

#[test]
fn later_native_provider_conflicts_name_fields_without_echoing_credentials() {
    let source = format!(
        r#"{PREPARE}
const cases=[
  (c,o)=>c.provider.openai.options.apiKey='later-private-key',
  (c,o)=>c.provider.openai.options.baseURL='https://later-private.invalid',
  (c,o)=>c.provider.openai.options.headers={{'x-api-key':'later-private-key'}},
  (c,o,m)=>m.headers={{Authorization:'later-private-key'}},
  (c,o)=>o.headers.Authorization='later-private-key',
  (c,o)=>c.provider.openai.options.api_keys={{openai:'later-private-key'}},
  (c,o)=>o.headers['X-Provider-API-Keys']='later-private-key'
];
const errors=[];
for(const change of cases){{
  const c=structuredClone(original), h=await create({{}}), o={{headers:{{}}}};
  await h.config(c); const model={{providerID:'openai',...c.provider.openai.models.token}};
  await h['chat.headers']({{provider:c.provider.openai,model}},o);change(c,o,model);
  try{{void o.headers;errors.push('missing conflict');}}
  catch(error){{errors.push(error.message);}}
}}
console.log(JSON.stringify(errors));"#
    );
    let returned = run_hook(&source, true);
    for message in returned.as_array().assert_value() {
        let message = message.as_str().assert_value();
        assert!(message.starts_with("Zeroshot OpenCode declared provider conflict:"));
        assert!(!message.contains("later-private"));
    }
}

#[test]
fn final_native_header_handoff_preserves_late_metadata_and_matching_authority() {
    let source = format!(
        r#"{PREPARE}
config.provider.openai.options.headers={{'HTTP-Referer':'https://opencode.ai','X-Title':'opencode','anthropic-beta':'native-feature'}};
config.provider.openai.options.headers.Authorization='Bearer declared-key';
input.model.headers.Authorization='Bearer declared-key';
const output={{headers:{{}}}};
await hooks['chat.headers'](input,output);
output.headers['X-Mutation']='retained';
const replacement={{...output.headers,'X-Replacement':'retained'}};
output.headers=replacement;
output.headers.authorization='Bearer declared-key';
const {{headers}}=output;
output.headers['X-After-Read']='retained';
console.log(JSON.stringify({{headers,sameObject:headers===replacement}}));"#
    );
    let returned = run_hook(&source, true);
    assert_eq!(returned["sameObject"], true);
    assert_eq!(returned["headers"]["Authorization"], "Bearer declared-key");
    assert!(returned["headers"].get("authorization").is_none());
    for name in ["X-Mutation", "X-Replacement", "X-After-Read"] {
        assert_eq!(returned["headers"][name], "retained");
    }
}

#[test]
fn captured_sdk_authority_is_checked_before_later_hooks_can_hide_a_conflict() {
    let source = format!(
        r#"{PREPARE}
config.provider.openai.options.apiKey='captured-private-key';
const output={{headers:{{}}}};let corrected=false,message='missing conflict';
try{{await hooks['chat.headers'](input,output);config.provider.openai.options.apiKey='declared-key';corrected=true;void output.headers;}}
catch(error){{message=error.message;}}
console.log(JSON.stringify({{corrected,message}}));"#
    );
    let returned = run_hook(&source, true);
    assert_eq!(returned["corrected"], false);
    assert!(
        returned["message"]
            .as_str()
            .assert_value()
            .contains("provider.options.apiKey")
    );
    assert!(!returned.to_string().contains("captured-private-key"));
}

#[test]
fn already_captured_sdk_header_case_aliases_are_not_mistaken_for_equivalent_headers() {
    let source = format!(
        r#"{PREPARE}
const errors=[];
for(const location of ['provider','model']){{
  const c=structuredClone(original),h=await create({{}});await h.config(c);
  const model={{providerID:'openai',...c.provider.openai.models.token}};
  if(location==='provider')c.provider.openai.options.headers={{authorization:'Bearer declared-key'}};
  else model.headers={{authorization:'Bearer declared-key'}};
  try{{await h['chat.headers']({{provider:c.provider.openai,model}},{{headers:{{}}}});errors.push('missing conflict');}}
  catch(error){{errors.push(error.message);}}
}}
console.log(JSON.stringify(errors));"#
    );
    for message in run_hook(&source, true).as_array().assert_value() {
        assert!(
            message
                .as_str()
                .assert_value()
                .starts_with("Zeroshot OpenCode declared provider conflict:")
        );
    }
}

#[test]
fn typed_native_upstream_byok_keys_are_redacted() {
    let environment = BTreeMap::from([("OPENCODE_CONFIG_CONTENT".to_owned(), json!({"provider":{"openrouter":{"options":{"api_keys":{"openai":"private-upstream-key"}}}}}).to_string())]);
    assert!(
        command::configuration_secrets(&environment).contains(&"private-upstream-key".to_owned())
    );
}

#[test]
fn native_file_login_mask_retains_wellknown_auth_and_filters_stale_entries() {
    let directory = TestDirectory::new("opencode-selective-native-login");
    let (_files, mut command) = hook_fixture(&directory, false, None);
    let data = directory.child("native-data");
    std::fs::create_dir_all(data.join("opencode")).assert_value();
    command.environment.insert(
        "XDG_DATA_HOME".to_owned(),
        data.to_string_lossy().into_owned(),
    );
    for selected in [
        json!({"type":"api","key":"wrong-stored-key"}),
        json!({"type":"oauth","refresh":"wrong-refresh","access":"wrong-access","expires":0}),
    ] {
        std::fs::write(data.join("opencode/auth.json"), json!({"openai":selected,
            "https://discovery.invalid":{"type":"wellknown","key":"DISCOVERY_TOKEN","token":"discovery-token"},
            "anthropic":{"type":"api","key":"other-native-key","metadata":{"owner":"native"},"ignored":"extra"},
            "stale":null,"invalid":{"type":"oauth","access":"bad"},"bad_metadata":{"type":"api","key":"bad","metadata":null}}).to_string()).assert_value();
        let auth = login::without_provider(None, &command, "openai")
            .assert_value()
            .masked;
        let auth: Value = serde_json::from_str(&auth).assert_value();
        assert!(auth.get("openai").is_none());
        assert_eq!(
            auth["https://discovery.invalid"]["token"],
            "discovery-token"
        );
        assert_eq!(auth["anthropic"]["metadata"]["owner"], "native");
        assert!(auth["anthropic"].get("ignored").is_none());
        for invalid in ["stale", "invalid", "bad_metadata"] {
            assert!(auth.get(invalid).is_none());
        }
        command
            .environment
            .insert("OPENCODE_AUTH_CONTENT".to_owned(), auth.to_string());
        let redactions = command::configuration_secrets(&command.environment);
        assert!(redactions.contains(&"discovery-token".to_owned()));
        assert!(redactions.contains(&"other-native-key".to_owned()));
    }
}

#[test]
fn authored_native_auth_keeps_native_unfiltered_json_semantics() {
    let directory = TestDirectory::new("opencode-auth-content-semantics");
    let (_files, command) = hook_fixture(&directory, false, None);
    let source = json!({"openai":{"type":"api","key":"wrong-key"},"native-extension":{"arbitrary":true},"stale":null}).to_string();
    let auth: Value = serde_json::from_str(
        &login::without_provider(Some(&source), &command, "openai")
            .assert_value()
            .masked,
    )
    .assert_value();
    assert!(auth.get("openai").is_none());
    assert_eq!(auth["native-extension"]["arbitrary"], true);
    assert_eq!(auth["stale"], Value::Null);
}

#[test]
fn native_auth_follows_regular_file_symlinks_and_refuses_special_files() {
    use std::os::unix::ffi::OsStrExt;
    let directory = TestDirectory::new("opencode-auth-file-boundary");
    let (_files, mut command) = hook_fixture(&directory, false, None);
    let data = directory.child("native-data");
    std::fs::create_dir_all(data.join("opencode")).assert_value();
    command.environment.insert(
        "XDG_DATA_HOME".to_owned(),
        data.to_string_lossy().into_owned(),
    );
    let source = directory.write(
        "native-auth.json",
        &json!({"anthropic":{"type":"api","key":"native-key"}}).to_string(),
    );
    let path = data.join("opencode/auth.json");
    std::os::unix::fs::symlink(source, &path).assert_value();
    assert!(
        login::without_provider(None, &command, "openai")
            .assert_value()
            .masked
            .contains("native-key")
    );
    std::fs::remove_file(&path).assert_value();
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).assert_value();
    // SAFETY: c_path is a valid terminated pathname, and the fixture owns its parent directory.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    assert!(login::without_provider(None, &command, "openai").is_err());
}
