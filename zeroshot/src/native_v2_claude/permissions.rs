//! Inspect Claude's resolved configuration before supplying a permission default.
//!
//! `get_settings` is the Agent SDK's configuration control request. Claude Code 2.1.237
//! accepts initialize/get_settings followed by EOF without a user turn. Unlike `--settings`,
//! which overrides user/project/local files, this reads the CLI's own merged policy.
//! See <https://code.claude.com/docs/en/settings> and
//! <https://code.claude.com/docs/en/cli-reference> for precedence and safe-mode semantics.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Value, json};

use super::ClaudeAdapter;
use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_capsule::provider_process::{
    ConfigurationRequest, PermissionPolicy, ProviderExecutionFiles, inspect_configuration,
};
use crate::native_v2_runner::{DriverControl, NodeRunnerError};

impl ClaudeAdapter {
    pub(super) async fn apply_permission_default(
        &self,
        files: Arc<ProviderExecutionFiles>,
        command: &mut ProcessSessionCommand,
        control: &DriverControl,
    ) -> Result<(), NodeRunnerError> {
        let permissive = if self.runners.is_hosted() {
            true
        } else {
            self.inspect_permission_policy(files, command.clone(), control)
                .await?
                == PermissionPolicy::Unset
        };
        if permissive {
            command
                .argv
                .push("--dangerously-skip-permissions".to_owned());
        }
        Ok(())
    }

    async fn inspect_permission_policy(
        &self,
        files: Arc<ProviderExecutionFiles>,
        mut command: ProcessSessionCommand,
        control: &DriverControl,
    ) -> Result<PermissionPolicy, NodeRunnerError> {
        // Transcript fixtures isolate model-turn I/O; configuration tests use real inspection.
        #[cfg(test)]
        if let Some(default) = self.test_permission_policy {
            return Ok(default);
        }
        if configured_arguments(&self.prefix_arguments)
            || configured_environment(&command.environment)
        {
            return Ok(PermissionPolicy::Configured);
        }
        command.argv.clone_from(&self.prefix_arguments);
        command.argv.extend([
            "--print".to_owned(),
            "--input-format".to_owned(),
            "stream-json".to_owned(),
            "--output-format".to_owned(),
            "stream-json".to_owned(),
            "--verbose".to_owned(),
            "--no-session-persistence".to_owned(),
            "--safe-mode".to_owned(),
            // Safe mode leaves managed hooks eligible. Disable hooks for this inspection,
            // without changing the permissions or sandbox settings being inspected.
            "--settings".to_owned(),
            concat!(
                r#"{"disableAllHooks":true,"apiKeyHelper":"","awsAuthRefresh":"","awsCredentialExport":"","#,
                r#""gcpAuthRefresh":"","proxyAuthHelper":""}"#,
            ).to_owned(),
        ]);
        let requests = vec![
            configuration_request("zeroshot-initialize", "initialize"),
            configuration_request("zeroshot-settings", "get_settings"),
        ];
        let Some(responses) = inspect_configuration(files, command, control, requests).await?
        else {
            return Ok(PermissionPolicy::Unavailable);
        };
        Ok(permission_policy(&responses))
    }
}

fn configuration_request(id: &'static str, subtype: &'static str) -> ConfigurationRequest {
    ConfigurationRequest {
        messages: vec![json!({
            "type": "control_request",
            "request_id": id,
            "request": { "subtype": subtype },
        })],
        response_pointer: "/response/request_id",
        response_id: json!(id),
    }
}

fn configured_arguments(arguments: &[String]) -> bool {
    arguments.iter().any(|argument| {
        matches!(
            argument.split('=').next(),
            Some(
                "--permission-mode"
                    | "--restricted"
                    | "--dangerously-skip-permissions"
                    | "--allow-dangerously-skip-permissions"
                    | "--allowedTools"
                    | "--allowed-tools"
                    | "--disallowedTools"
                    | "--disallowed-tools"
                    | "--permission-prompt-tool"
                    | "--settings"
                    | "--agent"
                    | "--agents"
            )
        )
    })
}

fn configured_environment(environment: &BTreeMap<String, String>) -> bool {
    environment
        .iter()
        .any(|(name, value)| permission_environment(name, value))
}

fn permission_environment(name: &str, value: &str) -> bool {
    match name {
        // Force-sandbox is present in the pinned CLI. Restricted mode is honored by newer
        // local CLIs. Never countermand either even when it is absent from merged settings.
        "CLAUDE_CODE_FORCE_SANDBOX"
        | "CLAUDE_CODE_RESTRICTED"
        | "CLAUDE_CODE_AUTO_MODE_EXTERNAL_PERMISSIONS" => {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        }
        "CLAUDE_BG_SESSION_PERMISSION_RULES" => !value.trim().is_empty(),
        _ => false,
    }
}

fn success(response: &Value) -> Option<&Value> {
    (response.get("type")?.as_str()? == "control_response"
        && response.pointer("/response/subtype")?.as_str()? == "success")
        .then(|| response.pointer("/response/response"))
        .flatten()
}

fn permission_policy(responses: &[Value]) -> PermissionPolicy {
    let [initialize, settings] = responses else {
        return PermissionPolicy::Unavailable;
    };
    let Some(initialize) = success(initialize) else {
        return PermissionPolicy::Unavailable;
    };
    let Some(mode) = initialize
        .get("current_permission_mode")
        .and_then(Value::as_str)
    else {
        return PermissionPolicy::Unavailable;
    };
    if mode != "default" {
        return PermissionPolicy::Configured;
    }
    let Some(settings) = success(settings) else {
        return PermissionPolicy::Unavailable;
    };
    settings_policy(settings)
}

fn settings_policy(settings: &Value) -> PermissionPolicy {
    if settings
        .get("errors")
        .is_some_and(|errors| errors.as_array().is_none_or(|errors| !errors.is_empty()))
    {
        return PermissionPolicy::Unavailable;
    }
    let Some(effective) = settings
        .get("effective")
        .filter(|settings| settings.is_object())
    else {
        return PermissionPolicy::Unavailable;
    };
    let Some(sources) = settings.get("sources").and_then(Value::as_array) else {
        return PermissionPolicy::Unavailable;
    };
    if sources.iter().any(|source| {
        !source
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.is_empty())
            || !source.get("settings").is_some_and(Value::is_object)
    }) {
        return PermissionPolicy::Unavailable;
    }
    if unconfigured_policy(effective)
        && sources
            .iter()
            .all(|source| unconfigured_policy(&source["settings"]))
    {
        PermissionPolicy::Unset
    } else {
        PermissionPolicy::Configured
    }
}

fn unconfigured_policy(settings: &Value) -> bool {
    let Some(settings) = settings.as_object() else {
        return false;
    };
    for key in ["permissions", "sandbox"] {
        if settings
            .get(key)
            .is_some_and(|value| value.as_object().is_none_or(|object| !object.is_empty()))
        {
            return false;
        }
    }
    if settings.contains_key("allowManagedPermissionRulesOnly") || settings.contains_key("agent") {
        return false;
    }
    settings.get("env").is_none_or(|environment| {
        environment.as_object().is_some_and(|environment| {
            environment.iter().all(|(name, value)| {
                value
                    .as_str()
                    .is_some_and(|value| !permission_environment(name, value))
            })
        })
    })
}

#[cfg(test)]
#[path = "permissions/tests.rs"]
mod tests;
