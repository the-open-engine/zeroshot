//! Envelope mechanics only. Each adapter owns its reserved native controls.
use openengine_cluster_protocol::HarnessConfiguration;
use serde_json::{Map, Value};
use crate::native_v2_runner::{DriverInvocation, NodeRunnerError};

pub(crate) fn args(config: &HarnessConfiguration) -> Result<Vec<String>, String> {
    if config.options().is_some() {
        return Err("harnessConfig.options is unsupported; use harnessConfig.args".to_owned());
    }
    Ok(config.args().unwrap_or_default())
}

pub(crate) fn options(config: &HarnessConfiguration) -> Result<Map<String, Value>, String> {
    if config.args().is_some() {
        return Err("harnessConfig.args is unsupported; use harnessConfig.options".to_owned());
    }
    Ok(config.options().cloned().unwrap_or_default())
}

/// The adapter selects its native parser's treatment of short option tokens.
#[derive(Clone, Copy)]
pub(crate) enum ShortOptionStyle {
    Joined,
    Exact,
}

pub(crate) fn reserved_args(
    args: &[String],
    reserved: &[&str],
    short_options: ShortOptionStyle,
) -> Result<(), String> {
    for arg in args {
        let flag = arg.split('=').next().unwrap_or(arg);
        if flag == "--"
            || reserved.iter().any(|owned| {
                if owned.starts_with("--") {
                    flag == *owned
                } else {
                    match short_options {
                        ShortOptionStyle::Exact => arg.as_str() == *owned,
                        ShortOptionStyle::Joined => {
                            flag == *owned || (owned.len() == 2 && flag.starts_with(owned))
                        }
                    }
                }
            })
        {
            return Err(format!("harnessConfig.args: {flag} is owned by Zeroshot"));
        }
    }
    if args.first().is_some_and(|arg| !arg.starts_with('-')) {
        return Err("harnessConfig.args must start with a native option".to_owned());
    }
    Ok(())
}

pub(crate) fn reserved_options(
    options: &Map<String, Value>,
    reserved: &[&str],
) -> Result<(), String> {
    for key in options.keys() {
        if reserved.contains(&key.as_str()) {
            return Err(format!("harnessConfig.options.{key} is owned by Zeroshot"));
        }
    }
    Ok(())
}

pub(crate) fn invocation_args(
    invocation: &DriverInvocation,
    harness: &str,
    validate: fn(&HarnessConfiguration) -> Result<(), String>,
) -> Result<Vec<String>, NodeRunnerError> {
    let Some(config) = invocation.node.binding.harness_config() else {
        return Ok(Vec::new());
    };
    validate(config).map_err(|detail| failure(invocation, harness, &detail))?;
    Ok(config.args().unwrap_or_default())
}

pub(crate) fn invocation_options(
    invocation: &DriverInvocation,
    harness: &str,
    validate: fn(&HarnessConfiguration) -> Result<(), String>,
) -> Result<Map<String, Value>, NodeRunnerError> {
    let Some(config) = invocation.node.binding.harness_config() else {
        return Ok(Map::new());
    };
    validate(config).map_err(|detail| failure(invocation, harness, &detail))?;
    Ok(config.options().cloned().unwrap_or_default())
}

pub(crate) fn failure(
    invocation: &DriverInvocation,
    harness: &str,
    detail: &str,
) -> NodeRunnerError {
    NodeRunnerError::DriverDetail(format!(
        "node `{}` harness `{harness}` harnessConfig: {detail}",
        invocation.node.reference.node
    ))
}

/// Attach identity after the adapter's existing bounded credential redaction.
pub(crate) fn context_error(
    invocation: &DriverInvocation,
    harness: &str,
    error: NodeRunnerError,
) -> NodeRunnerError {
    match error {
        NodeRunnerError::DriverDetail(detail)
            if invocation.node.binding.harness_config().is_some() =>
        {
            if detail.starts_with(&format!(
                "node `{}` harness `{harness}`",
                invocation.node.reference.node
            )) {
                NodeRunnerError::DriverDetail(detail)
            } else {
                NodeRunnerError::DriverDetail(format!(
                    "node `{}` harness `{harness}`: {detail}",
                    invocation.node.reference.node
                ))
            }
        }
        error => error,
    }
}

/// Only permission fallback consumes this evidence; arbitrary native args are never probed.
pub(crate) struct PermissionInput<'a> {
    pub(crate) files:
        std::sync::Arc<crate::native_v2_capsule::provider_process::ProviderExecutionFiles>,
    pub(crate) config: Option<&'a HarnessConfiguration>,
}

impl PermissionInput<'_> {
    pub(crate) fn has_overrides(&self) -> bool {
        self.config
            .is_some_and(|config| config.args().is_some_and(|args| !args.is_empty()))
    }
}

#[cfg(test)]
pub(crate) mod tests;
