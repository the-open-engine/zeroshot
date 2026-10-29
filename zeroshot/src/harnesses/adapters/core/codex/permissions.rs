//! Permission fallback resolved through Codex's native configuration stack.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};

use super::NativeV2CodexAdapter;
use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_capsule::provider_process::{
    ConfigurationRequest, PermissionPolicy, ProviderExecutionFiles, inspect_configuration,
    redaction_values,
};
use crate::native_v2_contract::{CodexProvider, EnvironmentVariableName};
use crate::native_v2_runner::{DriverControl, NodeRunnerError};

const MAX_NATIVE_PROVIDER_ENVIRONMENT: usize = 64;

struct CodexConfigurationInspection {
    permission_policy: PermissionPolicy,
    provider_environment: Vec<String>,
}

impl NativeV2CodexAdapter {
    pub(super) async fn apply_permission_default(
        &self,
        files: Arc<ProviderExecutionFiles>,
        command: &mut ProcessSessionCommand,
        control: &DriverControl,
    ) -> Result<Vec<String>, NodeRunnerError> {
        if self.runners.is_hosted() {
            command
                .argv
                .insert(1, "--dangerously-bypass-approvals-and-sandbox".to_owned());
            return Ok(Vec::new());
        }
        let inspection = self
            .inspect_configuration(files, command.clone(), control)
            .await?;
        let redactions =
            if self.config.local_user.is_some() && self.config.provider == CodexProvider::OpenAi {
                let declared_auth = command.environment.contains_key("CODEX_API_KEY")
                    || command.environment.contains_key("OPENAI_API_KEY");
                inherit_provider_environment(
                    &mut command.environment,
                    &inspection.provider_environment,
                    |name| {
                        if declared_auth && matches!(name, "CODEX_API_KEY" | "OPENAI_API_KEY") {
                            return None;
                        }
                        self.config.native_environment.get(name).cloned()
                    },
                )
            } else {
                Vec::new()
            };
        if inspection.permission_policy == PermissionPolicy::Unset {
            command
                .argv
                .insert(1, "--dangerously-bypass-approvals-and-sandbox".to_owned());
        }
        Ok(redactions)
    }

    async fn inspect_configuration(
        &self,
        files: Arc<ProviderExecutionFiles>,
        mut command: ProcessSessionCommand,
        control: &DriverControl,
    ) -> Result<CodexConfigurationInspection, NodeRunnerError> {
        // Transcript fixtures isolate model-turn I/O; configuration tests use real inspection.
        #[cfg(test)]
        if let Some(default) = self.test_permission_policy {
            return Ok(CodexConfigurationInspection {
                permission_policy: default,
                provider_environment: Vec::new(),
            });
        }
        let cwd = command.workspace.current_dir.clone();
        command.argv = vec!["app-server".to_owned()];
        let requests = vec![
            request(
                1,
                vec![json!({"id":1,"method":"initialize","params":{
                    "clientInfo":{"name":"zeroshot_config","version":env!("CARGO_PKG_VERSION")},
                    "capabilities":{"experimentalApi":true}
                }})],
            ),
            request(
                2,
                vec![
                    json!({"method":"initialized","params":{}}),
                    json!({"id":2,"method":"config/read","params":{"includeLayers":true,"cwd":cwd}}),
                ],
            ),
            request(
                3,
                vec![json!({"id":3,"method":"configRequirements/read","params":{}})],
            ),
        ];
        let Some(responses) = inspect_configuration(files, command, control, requests).await?
        else {
            return Ok(CodexConfigurationInspection {
                permission_policy: PermissionPolicy::Unavailable,
                provider_environment: Vec::new(),
            });
        };
        Ok(CodexConfigurationInspection {
            permission_policy: permission_policy(&responses, &cwd),
            provider_environment: provider_environment_names(&responses),
        })
    }
}

fn provider_environment_names(responses: &[Value]) -> Vec<String> {
    let Some((config, _, _)) = configuration_payloads(responses) else {
        return Vec::new();
    };
    let Some(provider) = config.get("model_provider").and_then(Value::as_str) else {
        return Vec::new();
    };
    let Some(settings) = config
        .get("model_providers")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(provider))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let names = settings
        .get("env_key")
        .and_then(Value::as_str)
        .into_iter()
        .chain(
            settings
                .get("env_http_headers")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|headers| headers.values().filter_map(Value::as_str)),
        );
    let names = names
        .filter_map(|name| EnvironmentVariableName::new(name).ok())
        .map(|name| name.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    if names.len() > MAX_NATIVE_PROVIDER_ENVIRONMENT {
        Vec::new()
    } else {
        names.into_iter().collect()
    }
}

fn inherit_provider_environment<F>(
    environment: &mut BTreeMap<String, String>,
    names: &[String],
    available: F,
) -> Vec<String>
where
    F: Fn(&str) -> Option<String>,
{
    let mut inherited = Vec::new();
    for name in names {
        if environment.contains_key(name) {
            continue;
        }
        if let Some(value) = available(name).filter(|value| !value.contains('\0')) {
            inherited.push(value.clone());
            environment.insert(name.clone(), value);
        }
    }
    redaction_values(inherited.iter().map(String::as_str))
}

fn request(id: u8, messages: Vec<Value>) -> ConfigurationRequest {
    ConfigurationRequest {
        messages,
        response_pointer: "/id",
        response_id: json!(id),
    }
}

fn permission_policy(responses: &[Value], cwd: &Path) -> PermissionPolicy {
    let Some((config, layers, requirements)) = configuration_payloads(responses) else {
        return PermissionPolicy::Unavailable;
    };
    if unconfigured_policy(config, cwd)
        && layers
            .iter()
            .all(|layer| unconfigured_policy(&layer["config"], cwd))
        && unconstrained(requirements)
    {
        PermissionPolicy::Unset
    } else {
        PermissionPolicy::Configured
    }
}

fn configuration_payloads(responses: &[Value]) -> Option<(&Value, &[Value], &Value)> {
    let [initialize, settings, requirements] = responses else {
        return None;
    };
    if !initialize.get("result").is_some_and(Value::is_object)
        || responses
            .iter()
            .any(|response| response.get("error").is_some())
    {
        return None;
    }
    let (config, layers) = configuration_layers(settings.get("result")?)?;
    let requirements = requirements.pointer("/result/requirements")?;
    (requirements.is_null() || requirements.is_object()).then_some((config, layers, requirements))
}

fn configuration_layers(settings: &Value) -> Option<(&Value, &[Value])> {
    let config = settings.get("config")?;
    // These fields are part of the config/read contract even when unset.
    config.get("sandbox_mode")?;
    config.get("approval_policy")?;
    settings.get("origins")?.as_object()?;
    let layers = settings.get("layers")?.as_array()?;
    layers
        .iter()
        .all(|layer| layer.get("config").is_some_and(Value::is_object))
        .then_some((config, layers))
}

fn unconfigured_policy(config: &Value, cwd: &Path) -> bool {
    let Some(config) = config.as_object() else {
        return false;
    };
    for key in [
        "sandbox_mode",
        "approval_policy",
        "approvals_reviewer",
        "sandbox_workspace_write",
        "permissions",
        "default_permissions",
        "profile",
        "auto_review",
        "browser_use",
        "computer_use",
    ] {
        if config.get(key).is_some_and(|value| !value.is_null()) {
            return false;
        }
    }
    // Codex promotes cached search to live under full access, even when cached was authored.
    // Leave that execution choice to Codex whenever the user requested cached search.
    if config
        .get("web_search")
        .is_some_and(|mode| mode == "cached")
        || !unconfigured_features(config.get("features"))
    {
        return false;
    }
    config
        .get("projects")
        .filter(|projects| !projects.is_null())
        .is_none_or(|projects| {
            projects.as_object().is_some_and(|projects| {
                projects.iter().all(|(path, project)| {
                    !cwd.starts_with(path)
                        || project
                            .get("trust_level")
                            .is_none_or(|trust| trust == "trusted")
                })
            })
        })
}

fn unconfigured_features(features: Option<&Value>) -> bool {
    features
        .filter(|features| !features.is_null())
        .is_none_or(|features| {
            features.as_object().is_some_and(|features| {
                features.get("web_search_cached") != Some(&Value::Bool(true))
                    && [
                        "network_proxy",
                        "guardian_approval",
                        "guardian_ext",
                        "write_stdin_approval",
                    ]
                    .iter()
                    .all(|name| features.get(*name).is_none_or(Value::is_null))
            })
        })
}

fn unconstrained(requirements: &Value) -> bool {
    if requirements.is_null() {
        return true;
    }
    requirements.as_object().is_some_and(|requirements| {
        requirements.iter().all(|(key, value)| {
            // Only known unrelated requirements can coexist with a permissive fallback.
            value.is_null()
                || matches!(
                    key.as_str(),
                    "mcpServers" | "apps" | "enforceResidency" | "allowedWebSearchModes"
                )
        })
    })
}

#[cfg(test)]
#[path = "permissions/tests.rs"]
mod tests;
