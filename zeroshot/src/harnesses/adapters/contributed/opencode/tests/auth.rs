use super::process::{adapter, invocation};
use super::*;
use crate::execution::process::{HostedProcessScope, ProcessSessionCommand};
use crate::native_v2_candidate::test_support::TestDirectory;

use std::sync::atomic::{AtomicUsize, Ordering};

static VERIFY_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn verify_script() -> String {
    format!(
        "{PLUGIN_SETUP}\n{}",
        r#"const http = hooks['session.http.request'];
const websocket = hooks['session.experimental.ws.handshake'];
const results = [];
for (const r of JSON.parse(process.env.REQUESTS)) {
  try {
    if (r.transport === 'ws') {
      await websocket({model:{providerID:r.provider}, kind:'primary', url:r.url, headers:r.headers});
    } else {
      await http({model:{providerID:r.provider}, kind:'primary', request:new Request(r.url, {method:'POST', headers:r.headers})});
    }
    results.push({ok:true});
  } catch (error) { results.push({ok:false, message:String(error.message)}); }
}
console.log(JSON.stringify(results));"#
    )
}

fn verify(provider: &str, fields: &[(&str, &str)], requests: Value) -> Vec<Value> {
    let name = format!(
        "opencode-verify-{}",
        VERIFY_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let directory = TestDirectory::new(&name);
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = provider.to_owned();
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let invocation = invocation(fields);
    let command: ProcessSessionCommand = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &invocation,
        files: &files,
        session_id: None,
    })
    .assert_value();
    let plugin_url = owned_plugin(&command, "zeroshot-provider-auth");
    let script = verify_script();
    let output = node_command()
        .args(["--input-type=module", "-e", &script, &plugin_url])
        .arg(&adapter.config.workspace)
        .envs(&command.environment)
        .env("REQUESTS", requests.to_string())
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
fn declared_key_installs_one_verifier_plugin_without_its_secret() {
    let name = format!(
        "opencode-verify-{}",
        VERIFY_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let directory = TestDirectory::new(&name);
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "openai".to_owned();
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let invocation = invocation(&[("OPENAI_API_KEY", "declared-key")]);
    let command: ProcessSessionCommand = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &invocation,
        files: &files,
        session_id: None,
    })
    .assert_value();

    let config: Value =
        serde_json::from_str(&command.environment["OPENCODE_CONFIG_CONTENT"]).assert_value();
    let plugins = config["plugins"].as_array().assert_value();
    assert_eq!(plugins.len(), 1);
    assert!(
        plugins[0]
            .as_str()
            .assert_value()
            .ends_with("/zeroshot-plugins/zeroshot-provider-auth/")
    );

    let plugin_dir = url::Url::parse(&owned_plugin(&command, "zeroshot-provider-auth"))
        .assert_value()
        .to_file_path()
        .assert_value();
    assert_eq!(
        std::fs::read_to_string(plugin_dir.join("package.json")).assert_value(),
        "{\"type\":\"module\"}"
    );
    assert!(
        !std::fs::read_to_string(plugin_dir.join("index.js"))
            .assert_value()
            .contains("declared-key")
    );
    assert!(!files.home().join("opencode").exists());

    let plugin_url = owned_plugin(&command, "zeroshot-provider-auth");
    let requests = json!([{
        "provider": "openai",
        "url": "https://openai.invalid/v1/chat/completions",
        "headers": {"authorization": "Bearer declared-key", "content-type": "application/json"}
    }]);
    let output = node_command()
        .args(["--input-type=module", "-e", &verify_script(), &plugin_url])
        .arg(&adapter.config.workspace)
        .envs(&command.environment)
        .env("REQUESTS", requests.to_string())
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let results: Vec<Value> = serde_json::from_slice(&output.stdout).assert_value();
    assert_eq!(results, vec![json!({"ok": true})]);

    let tmpdir = &command.environment["TMPDIR"];
    assert!(
        std::path::Path::new(tmpdir)
            .join("zeroshot-plugins")
            .join("zeroshot-provider-auth.loaded")
            .exists()
    );
    assert!(
        !std::path::Path::new(tmpdir)
            .join("zeroshot-plugins")
            .join("zeroshot-provider-auth.conflict")
            .exists(),
        "an accepted request must not leave a conflict behind"
    );
}

#[test]
fn a_conflict_leaves_only_the_message_for_the_node() {
    let name = format!(
        "opencode-verify-{}",
        VERIFY_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let directory = TestDirectory::new(&name);
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "openai".to_owned();
    let files =
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value();
    let invocation = invocation(&[
        ("OPENAI_API_KEY", "declared-key"),
        ("OPENAI_BASE_URL", "https://declared.invalid/v1"),
    ]);
    let command: ProcessSessionCommand = command::command(command::CommandRequest {
        adapter: &adapter,
        invocation: &invocation,
        files: &files,
        session_id: None,
    })
    .assert_value();
    let plugin_url = owned_plugin(&command, "zeroshot-provider-auth");
    let requests = json!([{
        "provider": "openai",
        "url": "https://declared.invalid/v1/chat/completions",
        "headers": {"authorization": "Bearer declared-key", "x-api-key": "file-header-secret"}
    }]);
    let output = node_command()
        .args(["--input-type=module", "-e", &verify_script(), &plugin_url])
        .arg(&adapter.config.workspace)
        .envs(&command.environment)
        .env("REQUESTS", requests.to_string())
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Vec<Value>>(&output.stdout).assert_value(),
        vec![
            json!({"ok": false, "message": "Zeroshot OpenCode declared provider conflict: x-api-key"})
        ]
    );

    let conflict = std::path::Path::new(&command.environment["TMPDIR"])
        .join("zeroshot-plugins")
        .join("zeroshot-provider-auth.conflict");
    assert!(conflict.exists(), "{conflict:?}");
    let text = std::fs::read_to_string(&conflict).assert_value();
    assert_eq!(
        text,
        "Zeroshot OpenCode declared provider conflict: x-api-key"
    );
    assert!(!text.contains("file-header-secret"), "{text}");
}

#[test]
fn verifier_accepts_only_the_declared_authority() {
    let requests = json!([
        {
            "provider": "openai",
            "url": "https://declared.invalid/v1/chat/completions",
            "headers": {
                "authorization": "Bearer declared-key",
                "content-type": "application/json",
                "x-session-id": "ses_fixture",
                "x-opencode-project": "fixture"
            }
        },
        {
            "provider": "openai",
            "url": "https://declared.invalid/v1/chat/completions",
            "headers": {
                "authorization": "Bearer declared-key",
                "x-api-key": "file-header-secret"
            }
        },
        {
            "provider": "openai",
            "url": "https://declared.invalid/v1/chat/completions",
            "headers": {"authorization": "Bearer wrong-secret"}
        },
        {
            "provider": "openai",
            "url": "https://declared.invalid/v1/chat/completions",
            "headers": {"content-type": "application/json"}
        },
        {
            "provider": "openai",
            "url": "https://other.invalid/v1/chat/completions",
            "headers": {"authorization": "Bearer declared-key"}
        },
        {
            "provider": "openai",
            "url": "https://declared.invalid/v10/chat",
            "headers": {"authorization": "Bearer declared-key"}
        },
        {
            "provider": "openai",
            "url": "https://declared.invalid/v1/chat/completions",
            "headers": {"authorization": "Bearer declared-key", "x-provider-api-keys": "upstream-secret"}
        },
        {
            "provider": "anthropic",
            "url": "https://declared.invalid/v1/chat/completions",
            "headers": {}
        }
    ]);
    let results = verify(
        "openai",
        &[
            ("OPENAI_API_KEY", "declared-key"),
            ("OPENAI_BASE_URL", "https://declared.invalid/v1"),
        ],
        requests,
    );
    let expected = vec![
        json!({"ok": true}),
        json!({"ok": false, "message": "Zeroshot OpenCode declared provider conflict: x-api-key"}),
        json!({"ok": false, "message": "Zeroshot OpenCode declared provider conflict: authorization"}),
        json!({"ok": false, "message": "Zeroshot OpenCode declared provider conflict: authorization"}),
        json!({"ok": false, "message": "Zeroshot OpenCode declared provider conflict: endpoint"}),
        json!({"ok": false, "message": "Zeroshot OpenCode declared provider conflict: endpoint"}),
        json!({"ok": false, "message": "Zeroshot OpenCode declared provider conflict: x-provider-api-keys"}),
        json!({"ok": true}),
    ];
    assert_eq!(results, expected);
    for secret in [
        "declared-key",
        "file-header-secret",
        "wrong-secret",
        "upstream-secret",
    ] {
        for result in &results {
            let text = result.to_string();
            assert!(!text.contains(secret), "{text}");
        }
    }
}

#[test]
fn verifier_checks_the_websocket_handshake() {
    let requests = json!([
        {
            "transport": "ws",
            "provider": "openai",
            "url": "wss://declared.invalid/v1/responses",
            "headers": {"Authorization": "Bearer declared-key", "openai-beta": "responses"}
        },
        {
            "transport": "ws",
            "provider": "openai",
            "url": "wss://declared.invalid/v1/responses",
            "headers": {"authorization": "Bearer declared-key", "x-api-key": "file-header-secret"}
        },
        {
            "transport": "ws",
            "provider": "openai",
            "url": "wss://other.invalid/v1/responses",
            "headers": {"authorization": "Bearer declared-key"}
        },
        {
            "transport": "ws",
            "provider": "anthropic",
            "url": "wss://declared.invalid/v1/responses",
            "headers": {}
        }
    ]);
    let results = verify(
        "openai",
        &[
            ("OPENAI_API_KEY", "declared-key"),
            ("OPENAI_BASE_URL", "https://declared.invalid/v1"),
        ],
        requests,
    );
    assert_eq!(
        results,
        vec![
            json!({"ok": true}),
            json!({"ok": false, "message": "Zeroshot OpenCode declared provider conflict: x-api-key"}),
            json!({"ok": false, "message": "Zeroshot OpenCode declared provider conflict: endpoint"}),
            json!({"ok": true}),
        ]
    );
    for secret in ["declared-key", "file-header-secret"] {
        for result in &results {
            let text = result.to_string();
            assert!(!text.contains(secret), "{text}");
        }
    }
}

#[test]
fn verifier_uses_each_lanes_canonical_header() {
    let results = verify(
        "anthropic",
        &[("ANTHROPIC_API_KEY", "declared-key")],
        json!([
            {
                "provider": "anthropic",
                "url": "https://api.anthropic.invalid/v1/messages",
                "headers": {"x-api-key": "declared-key"}
            },
            {
                "provider": "anthropic",
                "url": "https://api.anthropic.invalid/v1/messages",
                "headers": {"authorization": "Bearer declared-key"}
            }
        ]),
    );
    assert_eq!(
        results,
        vec![
            json!({"ok": true}),
            json!({"ok": false, "message": "Zeroshot OpenCode declared provider conflict: authorization, x-api-key"})
        ]
    );

    let results = verify(
        "gateway",
        &[
            ("GATEWAY_API_KEY", "declared-key"),
            ("GATEWAY_BASE_URL", "https://proxy.invalid/v1"),
            ("GATEWAY_API", "anthropic-messages"),
        ],
        json!([
            {
                "provider": "gateway",
                "url": "https://proxy.invalid/v1/messages",
                "headers": {"x-api-key": "declared-key"}
            }
        ]),
    );
    assert_eq!(results, vec![json!({"ok": true})]);

    let results = verify(
        "gateway",
        &[
            ("GATEWAY_API_KEY", "declared-key"),
            ("GATEWAY_BASE_URL", "https://proxy.invalid/v1"),
            ("GATEWAY_API", "openai-completions"),
        ],
        json!([
            {
                "provider": "gateway",
                "url": "https://proxy.invalid/v1/chat/completions",
                "headers": {"authorization": "Bearer declared-key"}
            },
            {
                "provider": "gateway",
                "url": "https://proxy.invalid/v1/chat/completions",
                "headers": {"x-api-key": "declared-key"}
            }
        ]),
    );
    assert_eq!(
        results,
        vec![
            json!({"ok": true}),
            json!({"ok": false, "message": "Zeroshot OpenCode declared provider conflict: authorization, x-api-key"})
        ]
    );
}

#[test]
fn verifier_without_a_declared_endpoint_leaves_the_url_native() {
    let results = verify(
        "openai",
        &[("OPENAI_API_KEY", "declared-key")],
        json!([
            {
                "provider": "openai",
                "url": "https://any.invalid/v1/chat/completions",
                "headers": {"authorization": "Bearer declared-key"}
            },
            {
                "provider": "openai",
                "url": "https://other.invalid/v2/models",
                "headers": {"authorization": "Bearer declared-key"}
            }
        ]),
    );
    assert_eq!(results, vec![json!({"ok": true}), json!({"ok": true})]);
}

#[test]
fn typed_native_upstream_byok_keys_are_redacted() {
    let environment = BTreeMap::from([("OPENCODE_CONFIG_CONTENT".to_owned(), json!({"provider":{"openrouter":{"options":{"api_keys":{"openai":"private-upstream-key"}}}}}).to_string())]);
    assert!(
        command::configuration_secrets(&environment).contains(&"private-upstream-key".to_owned())
    );
}
