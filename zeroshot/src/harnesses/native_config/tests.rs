use openengine_cluster_testkit::fixture::run_size;
use openengine_cluster_protocol::{
    HarnessConfiguration, NodeRuntimeBinding, NodeName, RuntimeLane, RuntimePlan,
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
            run_size("small"),
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
            run_size("small"),
            BTreeMap::from([(NodeName::new("work").unwrap(), binding)]),
        );
        registry.validate_runtime(&runtime).unwrap();
    }
}

#[cfg(feature = "contributed-harnesses")]
#[test]
fn opencode_pure_mode_is_rejected_at_admission() {
    let registry = crate::harnesses::HarnessRegistry::local();
    for flag in ["--pure", "--pure=true", "--pure=1", "--pure=false"] {
        for args in [json!([flag]), json!(["--title", "native title", flag])] {
            let runtime: RuntimePlan = serde_json::from_value(json!({
                "harness":"opencode", "provider":"anthropic", "size":"small", "nodes":{
                    "configured-node":{"kind":"agent","model":"opaque","harnessConfig":{"args":args}}
                }
            })).unwrap();
            let detail = registry.validate_runtime(&runtime).unwrap_err().to_string();
            for expected in [
                "configured-node",
                "opencode",
                "harnessConfig.args",
                "--pure",
            ] {
                assert!(detail.contains(expected), "{detail}");
            }
        }
    }
}

#[test]
fn hook_trust_workspace_and_transport_controls_are_rejected_at_admission() {
    let registry = crate::harnesses::HarnessRegistry::local();
    for (harness, provider, flags) in [
        (
            "codex",
            "openai",
            vec![
                "--dangerously-bypass-hook-trust",
                "--worktree",
                "--image",
                "-i",
            ],
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
        (
            "pi",
            "anthropic",
            vec!["--fork", "--no-extensions", "-ne", "--list-models"],
        ),
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
                    "args":[format!("--config=future_native_key=\"{value}\""), "--config", format!("another_native_key=\"{value}\"")]
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

#[test]
fn cli_admission_rejects_unbound_prompt_and_file_inputs() {
    let registry = crate::harnesses::HarnessRegistry::local();
    for (harness, provider, args) in [
        (
            "pi",
            "anthropic",
            json!(["--no-context-files", "CONFIG_PROMPT_1", "CONFIG_PROMPT_2"]),
        ),
        ("pi", "anthropic", json!(["-nc", "@caller-prompt.txt"])),
        (
            "pi",
            "anthropic",
            json!(["--tools", "read,grep", "CONFIG_PROMPT_1"]),
        ),
        (
            "pi",
            "anthropic",
            json!(["--future-extension=value", "CONFIG_PROMPT_1"]),
        ),
        (
            "codex",
            "openai",
            json!(["--skip-git-repo-check", "CONFIG_PROMPT_1"]),
        ),
        (
            "codex",
            "openai",
            json!(["--color", "never", "@caller-prompt.txt"]),
        ),
        (
            "claude",
            "anthropic",
            json!(["--disable-slash-commands", "CONFIG_PROMPT_1"]),
        ),
        (
            "claude",
            "anthropic",
            json!(["--debug", "mcp", "CONFIG_PROMPT_1"]),
        ),
        (
            "claude",
            "anthropic",
            json!(["--settings", "{}", "@caller-prompt.txt"]),
        ),
        (
            "opencode",
            "openai",
            json!(["--thinking", "CONFIG_PROMPT_1"]),
        ),
        (
            "opencode",
            "openai",
            json!(["--title", "native title", "CONFIG_PROMPT_1"]),
        ),
    ] {
        if !registry
            .descriptors()
            .any(|descriptor| descriptor.id == harness)
        {
            continue;
        }
        let runtime: RuntimePlan = serde_json::from_value(json!({
            "harness":harness, "provider":provider, "size":"small", "nodes":{
                "configured-node":{"kind":"agent","model":"opaque","harnessConfig":{"args":args}}
            }
        }))
        .unwrap();
        let detail = registry.validate_runtime(&runtime).unwrap_err().to_string();
        for expected in [
            "configured-node",
            harness,
            "harnessConfig.args[",
            "positional",
            "owned",
        ] {
            assert!(detail.contains(expected), "{detail}");
        }
        assert!(
            !detail.contains("CONFIG_PROMPT") && !detail.contains("caller-prompt"),
            "{detail}"
        );
    }
}

#[test]
fn cli_admission_keeps_bound_native_operands_and_unknown_option_names() {
    let registry = crate::harnesses::HarnessRegistry::local();
    for (harness, provider, args) in [
        (
            "pi",
            "anthropic",
            json!([
                "--tools",
                "-read",
                "--name",
                "@literal-name",
                "--future-extension=value"
            ]),
        ),
        (
            "pi",
            "anthropic",
            json!(["-xt", "read,grep", "-n", "native name"]),
        ),
        (
            "codex",
            "openai",
            json!([
                "-cweb_search=\"disabled\"",
                "--color",
                "never",
                "--future-option=value"
            ]),
        ),
        (
            "codex",
            "openai",
            json!(["--sandbox", "read-only", "--add-dir", "native directory"]),
        ),
        (
            "claude",
            "anthropic",
            json!([
                "--tools",
                "Read",
                "Grep",
                "Glob",
                "--settings",
                "{}",
                "--debug",
                "mcp"
            ]),
        ),
        (
            "claude",
            "anthropic",
            json!([
                "--tools=Read",
                "Grep",
                "--debug",
                "--no-chrome",
                "--future-option=value"
            ]),
        ),
        ("claude", "anthropic", json!(["--tools", ""])),
        (
            "opencode",
            "openai",
            json!([
                "--title",
                "native title",
                "--log-level",
                "DEBUG",
                "--future-option=value"
            ]),
        ),
    ] {
        if !registry
            .descriptors()
            .any(|descriptor| descriptor.id == harness)
        {
            continue;
        }
        let runtime: RuntimePlan = serde_json::from_value(json!({
            "harness":harness, "provider":provider, "size":"small", "nodes":{
                "configured-node":{"kind":"agent","model":"opaque","harnessConfig":{"args":args}}
            }
        }))
        .unwrap();
        registry
            .validate_runtime(&runtime)
            .unwrap_or_else(|error| panic!("{harness}: {error}"));
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
fn reserved_joined_credentials_report_the_option_without_its_value() {
    let registry = crate::harnesses::HarnessRegistry::local();
    if !registry
        .descriptors()
        .any(|descriptor| descriptor.id == "opencode")
    {
        return;
    }
    for option in ["-p", "-u"] {
        let runtime: RuntimePlan = serde_json::from_value(json!({
            "harness":"opencode", "provider":"openai", "size":"small", "nodes":{
                "configured-node":{"kind":"agent","model":"opaque","harnessConfig":{
                    "args":[format!("{option}private-value")]
                }}
            }
        }))
        .unwrap();
        let detail = registry.validate_runtime(&runtime).unwrap_err().to_string();
        assert!(
            detail.contains(option) && detail.contains("harnessConfig.args"),
            "{detail}"
        );
        assert!(!detail.contains("private-value"), "{detail}");
    }
}

#[test]
fn dangling_native_operands_cannot_consume_runtime_session_targets() {
    let registry = crate::harnesses::HarnessRegistry::local();
    for (harness, provider, args) in [
        ("codex", "openai", json!(["--config"])),
        ("claude", "anthropic", json!(["--settings"])),
        ("pi", "anthropic", json!(["--tools"])),
    ] {
        let runtime: RuntimePlan = serde_json::from_value(json!({
            "harness":harness, "provider":provider, "size":"small", "nodes":{
                "configured-node":{"kind":"agent","model":"opaque","harnessConfig":{"args":args}}
            }
        }))
        .unwrap();
        let detail = registry.validate_runtime(&runtime).unwrap_err().to_string();
        assert!(
            detail.contains("harnessConfig.args[0]") && detail.contains("requires an operand"),
            "{detail}"
        );
    }
}
