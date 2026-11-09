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
        let owned = reserved.iter().find(|owned| {
            if owned.starts_with("--") {
                flag == **owned
            } else {
                match short_options {
                    ShortOptionStyle::Exact => arg.as_str() == **owned,
                    ShortOptionStyle::Joined => {
                        flag == **owned || (owned.len() == 2 && flag.starts_with(**owned))
                    }
                }
            }
        });
        if let Some(owned) = owned.copied().or((flag == "--").then_some("--")) {
            return Err(format!("harnessConfig.args: {owned} is owned by Zeroshot"));
        }
    }
    Ok(())
}

/// Native token consumption, not an allowlist of option names or values.
pub(crate) enum ArgumentValues {
    One,
    Optional,
    Many,
}

/// Reject prompt/file positionals without guessing whether an unknown switch takes a value.
/// Adapters describe their native separated operands; other values must be joined to the option.
pub(crate) fn option_args(
    args: &[String],
    short_options: ShortOptionStyle,
    values: fn(&str) -> Option<ArgumentValues>,
) -> Result<(), String> {
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if !arg.starts_with('-') || matches!(arg.as_str(), "-" | "--") {
            return Err(format!(
                "harnessConfig.args[{index}]: positional prompt/file input is owned by Zeroshot; use --option=value to bind an operand for unrecognized native option syntax"
            ));
        }
        let raw_flag = arg.split('=').next().unwrap_or(arg);
        let flag = match short_options {
            ShortOptionStyle::Joined if !raw_flag.starts_with("--") => {
                raw_flag.get(..2).unwrap_or(raw_flag)
            }
            _ => raw_flag,
        };
        let attached = arg.contains('=') || flag.len() < raw_flag.len();
        match values(flag) {
            Some(ArgumentValues::One) if !attached => {
                if index + 1 == args.len() {
                    return Err(format!(
                        "harnessConfig.args[{index}]: native option requires an operand"
                    ));
                }
                index += 1;
            }
            Some(ArgumentValues::Optional) if !attached => {
                if args
                    .get(index + 1)
                    .is_some_and(|next| !next.starts_with('-'))
                {
                    index += 1;
                }
            }
            Some(ArgumentValues::Many) => {
                let start = index;
                while args
                    .get(index + 1)
                    .is_some_and(|next| !next.starts_with('-'))
                {
                    index += 1;
                }
                if !attached && index == start {
                    return Err(format!(
                        "harnessConfig.args[{start}]: native option requires an operand"
                    ));
                }
            }
            _ => {}
        }
        index += 1;
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
    /// The turn's credential redactions, applied to any reported inspection failure.
    pub(crate) redactions: &'a [String],
}

impl PermissionInput<'_> {
    pub(crate) fn has_overrides(&self) -> bool {
        self.config
            .is_some_and(|config| config.args().is_some_and(|args| !args.is_empty()))
    }
}

#[cfg(test)]
pub(crate) mod tests;
