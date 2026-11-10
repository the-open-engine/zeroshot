use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

#[test]
fn codex_trust_is_scoped_to_the_owned_session_handler_and_exact_definition() {
    // Independently cross-checked against native Codex 0.163 hooks/list and the
    // 0.155 source. Existing user/project hooks retain their own trust state.
    let handler = json!({
        "type":"command",
        "command":"'/opt/zeroshot' __active-memory-hook '/runtime/capability.json'",
        "timeout":1,
        "additionalContextLimit":crate::active_memory::MAX_INJECTION_BYTES,
    });
    let configuration = super::codex_configuration(&handler).assert_value();
    assert_eq!(configuration[0], "--config");
    assert_eq!(configuration[2], "--config");
    assert!(configuration[1].starts_with("hooks.PreToolUse="));
    let source = if cfg!(windows) {
        r"C:\<session-flags>\config.toml:pre_tool_use:0:0"
    } else {
        "/<session-flags>/config.toml:pre_tool_use:0:0"
    };
    assert_eq!(
        configuration[3],
        format!(
            "hooks.state={{{}={{\"trusted_hash\"=\"sha256:08c944b755e378b8c30b5b332a85903ae2e4b824e432e35b8083cbbafacba447\"}}}}",
            serde_json::to_string(source).assert_value(),
        ),
    );
    assert!(
        !configuration
            .iter()
            .any(|argument| argument.contains("bypass"))
    );
    let mut changed = handler;
    changed["command"] = json!("'/opt/zeroshot' __active-memory-hook '/runtime/other.json'");
    assert_ne!(
        configuration[3],
        super::codex_configuration(&changed).assert_value()[3]
    );
}
