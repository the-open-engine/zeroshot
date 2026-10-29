use super::*;
use crate::execution::process::{HostedProcessScope, ProcessSessionCommand};
use crate::native_v2_candidate::test_support::TestDirectory;

fn command_for(
    directory: &TestDirectory,
    native_auth: &str,
    declared: bool,
) -> (ProviderExecutionFiles, ProcessSessionCommand) {
    let mut adapter = super::process::adapter(directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "github".to_owned();
    adapter
        .local_environment
        .insert("OPENCODE_AUTH_CONTENT".to_owned(), native_auth.to_owned());
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let fields = if declared {
        vec![("COPILOT_GITHUB_TOKEN", "declared-github-token")]
    } else {
        vec![]
    };
    let command = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &super::process::invocation(&fields),
        files: &files,
        session_id: None,
    })
    .assert_value();
    (files, command)
}

#[test]
fn declared_github_token_uses_native_oauth_and_preserves_selected_enterprise_routing() {
    let directory = TestDirectory::new("opencode-github-authentication");
    let native = json!({"github-copilot":{"type":"oauth","refresh":"stored-refresh","access":"stored-access","expires":987654321,"enterpriseUrl":"company.ghe.com"},"other":{"type":"api","key":"unrelated-key"}}).to_string();
    let (_files, command) = command_for(&directory, &native, true);
    assert!(
        command
            .argv
            .iter()
            .any(|arg| arg == "github-copilot/opaque/model")
    );
    let selected: Value =
        serde_json::from_str(&command.environment["OPENCODE_AUTH_CONTENT"]).assert_value();
    assert_eq!(
        selected["github-copilot"],
        json!({"type":"oauth","refresh":"declared-github-token","access":"declared-github-token","expires":0,"enterpriseUrl":"company.ghe.com"})
    );
    assert_eq!(selected["other"]["key"], "unrelated-key");
    assert_eq!(command.environment[login::SOURCE_ENV], native);
    assert!(!command.environment["OPENCODE_CONFIG_CONTENT"].contains("declared-github-token"));
}

#[test]
fn github_native_login_keeps_its_original_authentication_and_no_override_plugin() {
    let directory = TestDirectory::new("opencode-github-native-login");
    let native = json!({"github-copilot":{"type":"oauth","refresh":"native-refresh","access":"native-access","expires":0,"enterpriseUrl":"company.ghe.com"}}).to_string();
    let (_files, command) = command_for(&directory, &native, false);
    assert_eq!(command.environment["OPENCODE_AUTH_CONTENT"], native);
    assert!(!command.environment.contains_key(login::SOURCE_ENV));
    assert!(!command.environment.contains_key("COPILOT_GITHUB_TOKEN"));
    let config: Value =
        serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    assert!(config.get("plugin").is_none());
}

fn native_fixture_plugin(directory: &TestDirectory) -> String {
    let source = r#"const nativeFetch=globalThis.fetch.bind(globalThis);
// Native Copilot discovery stays local to this synthetic fixture.
globalThis.fetch=(input,init)=>{
  const url=new URL(input instanceof Request?input.url:input);
  if(url.hostname==='api.githubcopilot.com'&&url.pathname==='/models')return Promise.resolve(new Response('{}',{status:401}));
  return nativeFetch(input,init);
};
export default async({client})=>{
  const set=client.auth.set.bind(client.auth);
  client.auth.set=async(...args)=>{
    const result=await set(...args);
    await new Promise(resolve=>setTimeout(resolve,2000));
    return result;
  };
  let pending;
  return {'chat.params':async()=>{
    if(pending)return;
    pending=client.auth.set({path:{id:'native-refresh-fixture'},body:{type:'oauth',refresh:'refreshed-unrelated',access:'refreshed-access',expires:10}});
    pending.catch(()=>{});
    // The model request races an unrelated native auth write, after it has reached the native route.
    await new Promise(resolve=>setTimeout(resolve,100));
  }};
};"#;
    url::Url::from_file_path(directory.write("native-github-fixture.mjs", source))
        .assert_value()
        .to_string()
}

#[tokio::test]
#[ignore = "requires installed OpenCode; localhost synthetic GitHub auth and native write concurrency"]
async fn native_github_request_keeps_declared_auth_while_unrelated_login_refresh_is_pending() {
    for inline in [false, true] {
        native_github_wire(inline).await;
    }
}

async fn native_github_wire(inline: bool) {
    let directory = TestDirectory::new("opencode-github-native-wire");
    let mut adapter = super::process::adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "github".to_owned();
    adapter.config.executable =
        std::env::var("ZEROSHOT_OPENCODE_BINARY").expect("explicit native binary");
    let (listener, endpoint) = super::native_authority::native_listener().await;
    let original = json!({"github-copilot":{"type":"oauth","refresh":"original-native-login","access":"original-native-access","expires":0},"unrelated":{"type":"api","key":"unrelated-key"}});
    let data = directory.child("data/opencode");
    std::fs::create_dir_all(&data).assert_value();
    std::fs::write(data.join("auth.json"), original.to_string()).assert_value();
    let config = json!({"plugin":[native_fixture_plugin(&directory)],"provider":{"github-copilot":{"options":{"baseURL":endpoint,"apiKey":"stored-native-key"},"models":{"opaque/model":{"id":"wire-fixture","limit":{"context":100000,"output":4096}}}}}});
    adapter.local_environment =
        super::native_authority::native_wire_environment(&directory, config);
    if inline {
        adapter
            .local_environment
            .insert("OPENCODE_AUTH_CONTENT".to_owned(), original.to_string());
    }
    let invocation = super::process::invocation(&[("COPILOT_GITHUB_TOKEN", "declared-wire-token")]);
    let captured = super::native_authority::run_native_wire(&adapter, invocation, listener).await;
    let captured = captured.to_ascii_lowercase();
    assert!(
        captured.contains("authorization: bearer declared-wire-token"),
        "{captured}"
    );
    assert!(
        captured.contains("x-initiator: user"),
        "native GitHub auth loader must run: {captured}"
    );
    assert!(!captured.contains("original-native-login"));
    let saved: Value =
        serde_json::from_slice(&std::fs::read(data.join("auth.json")).assert_value())
            .assert_value();
    assert_eq!(saved["github-copilot"], original["github-copilot"]);
    assert_eq!(
        saved["native-refresh-fixture"]["refresh"],
        "refreshed-unrelated"
    );
    assert!(!saved.to_string().contains("declared-wire-token"));
}
