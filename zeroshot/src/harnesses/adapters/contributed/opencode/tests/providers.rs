use super::*;
use crate::execution::process::{HostedProcessScope, ProcessSessionCommand};
use crate::native_v2_candidate::test_support::TestDirectory;

fn prepared(
    directory: &TestDirectory,
    provider: &str,
    fields: &[(&str, &str)],
) -> (ProviderExecutionFiles, ProcessSessionCommand) {
    let mut adapter = super::process::adapter(directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = provider.to_owned();
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let command = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &super::process::invocation(fields),
        files: &files,
        session_id: None,
    })
    .assert_value();
    (files, command)
}

fn run_plugin(command: &ProcessSessionCommand, source: &str) {
    let config: Value =
        serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    let plugin = config["plugin"]
        .as_array()
        .assert_value()
        .last()
        .assert_value()
        .as_str()
        .assert_value();
    let output = node_command()
        .args(["--input-type=module", "-e", source, plugin])
        .envs(&command.environment)
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn gateway_selects_native_sdk_and_preserves_native_model_settings() {
    for api in [
        "openai-responses",
        "openai-completions",
        "anthropic-messages",
    ] {
        let directory = TestDirectory::new("opencode-gateway-native");
        let (_files, command) = prepared(
            &directory,
            "gateway",
            &[
                ("GATEWAY_API_KEY", "selected-secret"),
                ("GATEWAY_BASE_URL", "https://proxy.example/authored/path"),
                ("GATEWAY_API", api),
            ],
        );
        assert!(command.argv.iter().any(|arg| arg == "gateway/opaque/model"));
        run_plugin(
            &command,
            r#"
const {default:create} = await import(process.argv[1]);
const hook = await create({client:{}});
const config = {permission:{bash:'deny'},plugin:['native'],provider:{gateway:{options:{apiKey:'old'},models:{'opaque/model':{limit:{context:98765,output:7654},provider:{npm:'@ai-sdk/wrong'}}}}}};
await hook.config(config);
const expected = {'openai-responses':'@ai-sdk/openai','openai-completions':'@ai-sdk/openai-compatible','anthropic-messages':'@ai-sdk/anthropic'}[process.env.GATEWAY_API];
const provider = config.provider.gateway, model = provider.models['opaque/model'];
if (provider.npm !== expected || model.provider.npm !== expected || model.limit.context !== 98765 || config.permission.bash !== 'deny' || config.plugin[0] !== 'native') throw Error('native settings changed');
if (provider.options.apiKey !== 'selected-secret' || provider.options.baseURL !== 'https://proxy.example/authored/path') throw Error('declared authority lost');
const input = {provider,model:{providerID:'gateway',api:{npm:expected},headers:{}}}, output={headers:{}};
await hook['chat.headers'](input,output);
input.model.api.npm = '@ai-sdk/wrong';
let refused=false;try {void output.headers;}catch {refused=true;}
if(!refused)throw Error('late native transport override accepted');
"#,
        );
    }
}

#[test]
fn bedrock_pins_bearer_region_and_native_endpoint_without_replacing_other_settings() {
    let directory = TestDirectory::new("opencode-bedrock-native");
    let (_files, command) = prepared(
        &directory,
        "bedrock",
        &[
            ("AWS_BEARER_TOKEN_BEDROCK", "selected-secret"),
            ("AWS_REGION", "us-east-1"),
            (
                "AWS_BEDROCK_BASE_URL",
                "https://bedrock.example/native/path",
            ),
        ],
    );
    assert!(
        command
            .argv
            .iter()
            .any(|arg| arg == "amazon-bedrock/opaque/model")
    );
    run_plugin(
        &command,
        r#"
const {default:create} = await import(process.argv[1]);
const hook = await create({client:{}});
const config={provider:{'amazon-bedrock':{options:{apiKey:'old',region:'wrong',endpoint:'https://wrong.example'},models:{'opaque/model':{limit:{output:4096}}}},other:{options:{apiKey:'unrelated'}}}};
await hook.config(config);
const provider=config.provider['amazon-bedrock'];
if(provider.options.apiKey!=='selected-secret' || provider.options.region!=='us-east-1' || provider.options.endpoint!=='https://bedrock.example/native/path' || provider.models['opaque/model'].limit.output!==4096 || config.provider.other.options.apiKey!=='unrelated')throw Error('native Bedrock authority/settings lost');
const output={headers:{}};
await hook['chat.headers']({provider,model:{providerID:'amazon-bedrock',headers:{}}},output);
provider.options.region='wrong';let refused=false;try{void output.headers;}catch{refused=true;}
if(!refused)throw Error('late region override accepted');
"#,
    );
}

fn ambient_adapter(directory: &TestDirectory, provider: &str, body: &str) -> OpenCodeAdapter {
    let mut config = super::process::adapter(directory, body).config;
    config.provider = provider.to_owned();
    config.native_environment = LocalHarnessEnvironment::new(BTreeMap::from([
        (
            "AWS_BEARER_TOKEN_BEDROCK".to_owned(),
            "synthetic-bearer-secret".to_owned(),
        ),
        (
            "AWS_SECRET_ACCESS_KEY".to_owned(),
            "synthetic-iam-secret".to_owned(),
        ),
        ("AWS_PROFILE".to_owned(), "native-profile".to_owned()),
        (
            "AWS_WEB_IDENTITY_TOKEN_FILE".to_owned(),
            "/native/web-token".to_owned(),
        ),
    ]));
    OpenCodeAdapter::new_local(config).assert_value()
}

#[test]
fn native_aws_authentication_is_preserved_only_for_the_bedrock_lane() {
    for provider in ["openai", "bedrock"] {
        let directory = TestDirectory::new("opencode-aws-context");
        let adapter = ambient_adapter(&directory, provider, "#!/bin/sh\nexit 0\n");
        if provider == "bedrock" {
            assert_eq!(adapter.local_environment["AWS_PROFILE"], "native-profile");
            assert_eq!(
                adapter.local_environment["AWS_WEB_IDENTITY_TOKEN_FILE"],
                "/native/web-token"
            );
        } else {
            assert!(
                !adapter
                    .local_environment
                    .keys()
                    .any(|name| name.starts_with("AWS_"))
            );
        }
    }
}

#[tokio::test]
async fn native_aws_errors_redact_ambient_bearer_and_iam_secrets() {
    let directory = TestDirectory::new("opencode-aws-redaction");
    let adapter = ambient_adapter(
        &directory,
        "bedrock",
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' \"$AWS_BEARER_TOKEN_BEDROCK $AWS_SECRET_ACCESS_KEY\" >&2\nexit 1\n",
    );
    let error = super::process::run(&adapter, super::process::invocation(&[]))
        .await
        .unwrap_err()
        .to_string();
    assert!(!error.contains("synthetic-bearer-secret"));
    assert!(!error.contains("synthetic-iam-secret"));
    assert!(error.contains("[REDACTED]"));
}
