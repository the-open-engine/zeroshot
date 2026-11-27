//! Managed sessions keep the working directory Zeroshot assigned.
use super::process::{adapter, invocation};
use super::*;
use crate::execution::process::{HostedProcessScope, ProcessSessionCommand};
use crate::native_v2_candidate::test_support::TestDirectory;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn an_empty_configuration_denies_session_move() {
    let mut config = json!({});
    command::deny_session_move(&mut config).assert_value();
    assert_eq!(
        config,
        json!({
            "permission": {"opencode_session_move": "deny"},
            "permissions": [
                {"action": "opencode_session_move", "resource": "*", "effect": "deny"}
            ]
        })
    );
}

#[test]
fn a_string_permission_becomes_the_wildcard_rule() {
    let mut config = json!({"permission": "allow"});
    command::deny_session_move(&mut config).assert_value();
    assert_eq!(
        config,
        json!({
            "permission": {"*": "allow", "opencode_session_move": "deny"},
            "permissions": [
                {"action": "opencode_session_move", "resource": "*", "effect": "deny"}
            ]
        })
    );
}

#[test]
fn a_node_permission_keeps_its_rules_but_cannot_allow_session_move() {
    let mut config = json!({"permission": {"bash": "ask", "opencode_session_move": "allow"}});
    command::deny_session_move(&mut config).assert_value();
    assert_eq!(
        config,
        json!({
            "permission": {"bash": "ask", "opencode_session_move": "deny"},
            "permissions": [
                {"action": "opencode_session_move", "resource": "*", "effect": "deny"}
            ]
        })
    );
}

#[test]
fn any_other_permission_value_is_refused() {
    for permission in [json!(["allow"]), json!(1), json!(true)] {
        let mut config = json!({"permission": permission});
        let error = command::deny_session_move(&mut config).expect_err("must refuse");
        assert!(
            matches!(&error, NodeRunnerError::DriverDetail(detail) if detail.contains("permission")),
            "{error:?}"
        );
    }
}

#[test]
fn native_rules_keep_their_order_and_end_with_the_deny() {
    let mut config = json!({
        "permissions": [
            {"action": "*", "resource": "*", "effect": "allow"},
            {"action": "opencode_session_move", "resource": "*", "effect": "allow"}
        ]
    });
    command::deny_session_move(&mut config).assert_value();
    assert_eq!(
        config["permissions"],
        json!([
            {"action": "*", "resource": "*", "effect": "allow"},
            {"action": "opencode_session_move", "resource": "*", "effect": "allow"},
            {"action": "opencode_session_move", "resource": "*", "effect": "deny"}
        ])
    );
}

#[test]
fn every_inline_agent_ends_with_the_deny() {
    let mut config = json!({
        "agents": {
            "build": {"permissions": [{"action": "opencode_session_move", "resource": "*", "effect": "allow"}]},
            "plan": {},
            "broken": "x"
        }
    });
    command::deny_session_move(&mut config).assert_value();
    assert_eq!(
        config["agents"]["build"]["permissions"],
        json!([
            {"action": "opencode_session_move", "resource": "*", "effect": "allow"},
            {"action": "opencode_session_move", "resource": "*", "effect": "deny"}
        ])
    );
    assert_eq!(
        config["agents"]["plan"]["permissions"],
        json!([{"action": "opencode_session_move", "resource": "*", "effect": "deny"}])
    );
    assert_eq!(config["agents"]["broken"], json!("x"));
    assert_eq!(config["agents"].as_object().assert_value().len(), 3);
}

#[test]
fn legacy_agents_and_modes_deny_session_move() {
    let mut config = json!({
        "agent": {"build": {"permission": {"bash": "ask", "opencode_session_move": "allow"}}},
        "mode": {"review": {"permission": "allow"}}
    });
    command::deny_session_move(&mut config).assert_value();
    assert_eq!(
        config["agent"]["build"]["permission"],
        json!({"bash": "ask", "opencode_session_move": "deny"})
    );
    assert_eq!(
        config["mode"]["review"]["permission"],
        json!({"*": "allow", "opencode_session_move": "deny"})
    );
}

#[test]
fn a_non_array_native_permissions_value_is_refused() {
    for permissions in [json!({"x": 1}), json!("allow")] {
        let mut config = json!({"permissions": permissions});
        let error = command::deny_session_move(&mut config).expect_err("must refuse");
        assert!(
            matches!(&error, NodeRunnerError::DriverDetail(detail) if detail.contains("permissions")),
            "{error:?}"
        );
    }
    let mut config = json!({"agents": {"build": {"permissions": 1}}});
    let error = command::deny_session_move(&mut config).expect_err("must refuse");
    assert!(
        matches!(&error, NodeRunnerError::DriverDetail(detail) if detail.contains("permissions")),
        "{error:?}"
    );
}

#[test]
fn the_command_configuration_denies_session_move() {
    let directory = TestDirectory::new("opencode-session-move-command");
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
    assert_eq!(config["permission"]["opencode_session_move"], json!("deny"));
    assert_eq!(
        config["permissions"].as_array().assert_value().last(),
        Some(&json!({"action": "opencode_session_move", "resource": "*", "effect": "deny"}))
    );
}

const PROBE: &str = r#"const found = await search({ query: "session" }); const paths = (found.items || []).map((item) => item.path); let moved; try { moved = await (tools.opencode.session && tools.opencode.session.move ? tools.opencode.session.move({ directory: "." }) : tools.opencode.session_move({ directory: "." })); } catch (error) { moved = "ERR " + String((error && error.message) || error); } return JSON.stringify({ paths, moved });"#;

pub(super) fn sse(blocks: &[(Value, Value)], stop: &str) -> String {
    let mut events = vec![
        json!({"type":"message_start","message":{"id":"msg_probe","type":"message","role":"assistant","model":"probe-model","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}}),
    ];
    for (index, (start, delta)) in blocks.iter().enumerate() {
        events.push(json!({"type":"content_block_start","index":index,"content_block":start}));
        events.push(json!({"type":"content_block_delta","index":index,"delta":delta}));
        events.push(json!({"type":"content_block_stop","index":index}));
    }
    events.push(json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},"usage":{"output_tokens":1}}));
    events.push(json!({"type":"message_stop"}));
    events
        .iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

async fn read_body(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    loop {
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if bytes.len() >= end + 4 + length {
                return Some(
                    String::from_utf8_lossy(&bytes[end + 4..end + 4 + length]).into_owned(),
                );
            }
        }
        let mut chunk = [0; 8192];
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 || bytes.len() > 8 * 1024 * 1024 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

/// A provider that answers the first tool-carrying request with an `execute` call running
/// `PROBE`, and every other request with plain text. Requests with a tool result are sent out.
async fn serve(
    listener: tokio::net::TcpListener,
    results: tokio::sync::mpsc::UnboundedSender<String>,
) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let results = results.clone();
        tokio::spawn(async move {
            let Some(body) = read_body(&mut stream).await else {
                return;
            };
            let request: Value = serde_json::from_str(&body).unwrap_or_default();
            let text = |text: &str| {
                sse(
                    &[(
                        json!({"type":"text","text":""}),
                        json!({"type":"text_delta","text":text}),
                    )],
                    "end_turn",
                )
            };
            let payload = if body.contains("\"tool_result\"") {
                let _ = results.send(body.clone());
                text("done")
            } else if request.get("tools").is_some() {
                sse(
                    &[(
                        json!({"type":"tool_use","id":"toolu_probe","name":"execute","input":{}}),
                        json!({"type":"input_json_delta","partial_json": json!({"code": PROBE}).to_string()}),
                    )],
                    "tool_use",
                )
            } else {
                text("Probe")
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}

async fn native_session_move_probe(extra: Value) {
    let directory = TestDirectory::new("opencode-native-session-move");
    let mut adapter = adapter(&directory, "#!/bin/sh\nexit 0\n");
    adapter.config.provider = "anthropic".to_owned();
    adapter.config.executable =
        std::env::var("ZEROSHOT_OPENCODE_BINARY").expect("set the explicit native OpenCode binary");
    let mut config = json!({"provider":{"anthropic":{"options":{"apiKey":"stored-key"},"models":{"opaque/model":{"id":"probe-model","name":"Probe","limit":{"context":100000,"output":4096}}}}}});
    if let Some(extra) = extra.as_object() {
        if let Some(config) = config.as_object_mut() {
            for (key, value) in extra {
                config.insert(key.clone(), value.clone());
            }
        }
    }
    adapter.local_environment =
        super::native_authority::native_wire_environment(&directory, config);
    let (listener, endpoint) = super::native_authority::native_listener().await;
    let (sender, mut results) = tokio::sync::mpsc::unbounded_channel();
    let server = tokio::spawn(serve(listener, sender));
    let invocation = invocation(&[
        ("ANTHROPIC_API_KEY", "declared-probe-key"),
        ("ANTHROPIC_BASE_URL", endpoint.as_str()),
    ]);
    let (_cancel, control, mut durable) = crate::native_v2_runner::test_support::control();
    let drain = tokio::spawn(async move { while durable.recv_output().await.is_ok() {} });
    let _result = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        adapter.run(invocation, control),
    )
    .await
    .assert_value();
    server.abort();
    drain.await.assert_value();
    let body = results.try_recv().assert_value();
    let request: Value = serde_json::from_str(&body).assert_value();
    let result = request["messages"]
        .as_array()
        .assert_value()
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .find(|part| part["type"] == "tool_result")
        .map(|part| part["content"].to_string())
        .assert_value();
    assert!(result.contains("tools.opencode.session.rename"), "{result}");
    assert!(!result.contains("tools.opencode.session.move"), "{result}");
    assert!(result.contains("ERR Unknown tool"), "{result}");
}

#[tokio::test]
#[ignore = "requires installed OpenCode 2.4 or newer; localhost provider and a dummy key only"]
async fn native_session_move_is_absent_from_search_and_cannot_be_called() {
    native_session_move_probe(json!({})).await;
}

#[tokio::test]
#[ignore = "requires installed OpenCode 2.4 or newer; localhost provider and a dummy key only"]
async fn native_later_allow_rules_cannot_bring_session_move_back() {
    native_session_move_probe(json!({
        "permissions": [
            {"action": "*", "resource": "*", "effect": "allow"},
            {"action": "opencode_session_move", "resource": "*", "effect": "allow"}
        ],
        "agents": {
            "build": {
                "permissions": [
                    {"action": "opencode_session_move", "resource": "*", "effect": "allow"}
                ]
            }
        }
    }))
    .await;
}
