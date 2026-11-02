use openengine_cluster_protocol::{
    HarnessConfiguration, NodeRuntimeBinding, NodeName, RuntimeLane, RuntimePlan, RunSize,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(crate) fn set(binding: &mut NodeRuntimeBinding, value: Value) {
    let NodeRuntimeBinding::Agent { harness_config, .. } = binding else {
        panic!("agent binding required")
    };
    *harness_config = Some(HarnessConfiguration::new(value).unwrap());
}

#[test]
fn native_configuration_admission_reports_node_harness_and_reserved_path() {
    let registry = crate::harnesses::HarnessRegistry::local();
    for (harness, provider, config, path) in [
        (
            "codex",
            "openai",
            json!({"args":["--model=foreign"]}),
            "--model",
        ),
        (
            "codex",
            "openai",
            json!({"args":["-cmodel_providers.foreign.api_key=secret"]}),
            "model_providers",
        ),
        (
            "claude",
            "anthropic",
            json!({"args":["--settings", "{\"env\":{\"ANTHROPIC_API_KEY\":\"secret\"}}"]}),
            "env",
        ),
        (
            "claude",
            "anthropic",
            json!({"args":["--json-schema={}"]}),
            "--json-schema",
        ),
        (
            "copilot",
            "github",
            json!({"options":{"hooks":false}}),
            "hooks",
        ),
        (
            "pi",
            "anthropic",
            json!({"args":["--session-id=foreign"]}),
            "--session-id",
        ),
        (
            "opencode",
            "openai",
            json!({"options":{"provider":{}}}),
            "provider",
        ),
        (
            "hermes",
            "openai",
            json!({"options":{"model":"foreign"}}),
            "model",
        ),
    ] {
        if !registry
            .descriptors()
            .any(|descriptor| descriptor.id == harness)
        {
            continue;
        }
        let mut binding = crate::native_v2_runner::test_support::binding(
            crate::execution::SessionScope::Execution,
        );
        set(&mut binding, config);
        let runtime = RuntimePlan::new(
            RuntimeLane::new(harness, provider).unwrap(),
            RunSize::Small,
            BTreeMap::from([(NodeName::new("configured-node").unwrap(), binding)]),
        );
        let detail = registry.validate_runtime(&runtime).unwrap_err().to_string();
        for expected in ["configured-node", harness, "harnessConfig", path] {
            assert!(detail.contains(expected), "{detail}");
        }
        assert!(!detail.contains("=secret"));
    }
}

#[test]
fn unknown_native_json_options_are_left_to_native_semantics() {
    let registry = crate::harnesses::HarnessRegistry::local();
    for (harness, provider) in [
        ("copilot", "github"),
        ("hermes", "openai"),
        ("opencode", "openai"),
    ] {
        if !registry
            .descriptors()
            .any(|descriptor| descriptor.id == harness)
        {
            continue;
        }
        let mut binding = crate::native_v2_runner::test_support::binding(
            crate::execution::SessionScope::Execution,
        );
        set(
            &mut binding,
            json!({"options":{"futureNativeOption":{"value":true}}}),
        );
        let runtime = RuntimePlan::new(
            RuntimeLane::new(harness, provider).unwrap(),
            RunSize::Small,
            BTreeMap::from([(NodeName::new("work").unwrap(), binding)]),
        );
        registry.validate_runtime(&runtime).unwrap();
    }
}

#[test]
fn hook_trust_workspace_and_transport_controls_are_rejected_at_admission() {
    let registry = crate::harnesses::HarnessRegistry::local();
    for (harness, provider, flags) in [
        (
            "codex",
            "openai",
            vec!["--dangerously-bypass-hook-trust", "--worktree"],
        ),
        (
            "claude",
            "anthropic",
            vec![
                "--safe-mode",
                "--background",
                "--bg",
                "--cloud",
                "--environment",
                "--desktop",
                "--remote-control",
            ],
        ),
        ("pi", "anthropic", vec!["--fork", "--no-extensions", "-ne"]),
    ] {
        for flag in flags {
            for args in [
                json!([flag, "native-value"]),
                json!([format!("{flag}=native-value")]),
            ] {
                // Pi's short aliases are exact tokens, including when they take no value.
                if flag == "-ne" && args[0] != flag {
                    continue;
                }
                let runtime: RuntimePlan = serde_json::from_value(json!({
                    "harness":harness, "provider":provider, "size":"small", "nodes":{
                        "configured-node":{"kind":"agent","model":"opaque","harnessConfig":{"args":args}}
                    }
                })).unwrap();
                let detail = registry.validate_runtime(&runtime).unwrap_err().to_string();
                for expected in ["configured-node", harness, "harnessConfig.args", flag] {
                    assert!(detail.contains(expected), "{detail}");
                }
            }
        }
    }
}

#[test]
fn pi_native_tool_modifier_values_are_preserved_at_admission() {
    let registry = crate::harnesses::HarnessRegistry::local();
    for args in [json!(["--tools", "-read"]), json!(["-t", "-read,-bash"])] {
        let runtime: RuntimePlan = serde_json::from_value(json!({
            "harness":"pi", "provider":"anthropic", "size":"small", "nodes":{
                "configured-node":{"kind":"agent","model":"opaque","harnessConfig":{"args":args}}
            }
        }))
        .unwrap();
        registry.validate_runtime(&runtime).unwrap();
        assert_eq!(
            runtime
                .nodes()
                .values()
                .next()
                .unwrap()
                .harness_config()
                .unwrap()
                .args()
                .unwrap(),
            serde_json::from_value::<Vec<String>>(args).unwrap()
        );
    }
}

#[test]
fn native_short_option_styles_keep_joined_controls_and_pi_values_distinct() {
    use super::{ShortOptionStyle, reserved_args};
    assert!(reserved_args(&["-mforeign".into()], &["-m"], ShortOptionStyle::Joined).is_err());
    assert!(
        reserved_args(
            &["--tools".into(), "-read".into()],
            &["-r"],
            ShortOptionStyle::Exact
        )
        .is_ok()
    );
    assert!(reserved_args(&["-r".into()], &["-r"], ShortOptionStyle::Exact).is_err());
}

#[test]
fn codex_subcommands_cannot_redirect_execution_at_admission() {
    let registry = crate::harnesses::HarnessRegistry::local();
    for command in ["resume", "fork", "review", "help"] {
        for args in [
            json!([command, "foreign-thread"]),
            json!(["--color", "never", command, "foreign-thread"]),
        ] {
            let runtime: RuntimePlan = serde_json::from_value(json!({
                "harness":"codex", "provider":"openai", "size":"small", "nodes":{
                    "configured-node":{"kind":"agent","model":"opaque","harnessConfig":{"args":args}}
                }
            })).unwrap();
            let detail = registry.validate_runtime(&runtime).unwrap_err().to_string();
            for expected in ["configured-node", "codex", "harnessConfig.args", command] {
                assert!(detail.contains(expected), "{detail}");
            }
            assert!(!detail.contains("foreign-thread"));
        }
    }
}

#[test]
fn codex_literal_option_values_can_use_subcommand_names() {
    let registry = crate::harnesses::HarnessRegistry::local();
    for value in ["resume", "fork", "review", "help"] {
        let runtime: RuntimePlan = serde_json::from_value(json!({
            "harness":"codex", "provider":"openai", "size":"small", "nodes":{
                "configured-node":{"kind":"agent","model":"opaque","harnessConfig":{
                    "args":[format!("--image={value}"), "--config", format!("future_native_key=\"{value}\"")]
                }}
            }
        })).unwrap();
        registry.validate_runtime(&runtime).unwrap();
    }
}

#[test]
fn claude_prompt_files_and_session_selectors_are_rejected_at_admission() {
    let registry = crate::harnesses::HarnessRegistry::local();
    for flag in [
        "--system-prompt-file",
        "--append-system-prompt-file",
        "--from-pr",
        "--teleport",
    ] {
        for args in [
            json!([flag, "native-value"]),
            json!([format!("{flag}=native-value")]),
        ] {
            let runtime: RuntimePlan = serde_json::from_value(json!({
                "harness":"claude", "provider":"anthropic", "size":"small", "nodes":{
                    "configured-node":{"kind":"agent","model":"opaque","harnessConfig":{"args":args}}
                }
            })).unwrap();
            let detail = registry.validate_runtime(&runtime).unwrap_err().to_string();
            for expected in ["configured-node", "claude", "harnessConfig.args", flag] {
                assert!(detail.contains(expected), "{detail}");
            }
            assert!(!detail.contains("native-value"));
        }
    }
}
