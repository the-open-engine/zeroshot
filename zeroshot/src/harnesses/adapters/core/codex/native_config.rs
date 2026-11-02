use openengine_cluster_protocol::HarnessConfiguration;
use crate::harnesses::native_config;

pub(crate) fn validate(config: &HarnessConfiguration) -> Result<(), String> {
    let args = native_config::args(config)?;
    // Clap recognizes subcommands even after a prompt positional or another option.
    // Keep command/session selection runtime-owned without parsing every native option.
    if let Some(command) = args
        .iter()
        .find(|arg| matches!(arg.as_str(), "resume" | "fork" | "review" | "help"))
    {
        return Err(format!(
            "harnessConfig.args: Codex exec subcommand `{command}` is owned by Zeroshot; use --option=value for a literal option value"
        ));
    }
    native_config::reserved_args(
        &args,
        &[
            "--model",
            "--provider",
            "--api-key",
            "-m",
            "--oss",
            "--local-provider",
            "--profile",
            "-p",
            "--cd",
            "-C",
            "--json",
            "--output-schema",
            "--output-last-message",
            "-o",
            "--session",
            "--last",
            "--all",
            "--ephemeral",
            "--dangerously-bypass-hook-trust",
            "--worktree",
            "--remote",
            "--listen",
            "--help",
            "-h",
            "--version",
            "-V",
        ],
        native_config::ShortOptionStyle::Joined,
    )?;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let setting = if arg == "--config" || arg == "-c" {
            iter.next().map(String::as_str)
        } else {
            arg.strip_prefix("--config=")
                .or_else(|| arg.strip_prefix("-c").filter(|_| !arg.starts_with("--")))
        };
        if arg == "--disable"
            || arg == "--enable"
            || arg.starts_with("--disable=")
            || arg.starts_with("--enable=")
        {
            let feature = arg
                .split_once('=')
                .map(|(_, feature)| feature)
                .or_else(|| iter.next().map(String::as_str));
            if feature.is_some_and(|feature| matches!(feature, "codex_hooks" | "hooks")) {
                return Err("harnessConfig.args: owned hook features cannot be changed".to_owned());
            }
        }
        if let Some(setting) = setting {
            let setting = setting.trim_start_matches('=');
            let key = setting.split('=').next().unwrap_or(setting).trim();
            let root = key
                .split('.')
                .next()
                .unwrap_or(key)
                .trim_matches(['\'', '"', ' ']);
            if [
                "model",
                "model_provider",
                "profile",
                "profiles",
                "model_providers",
                "model_reasoning_effort",
                "model_instructions_file",
                "developer_instructions",
                "instructions",
                "base_instructions",
                "cli_auth_credentials_store",
                "forced_login_method",
                "forced_chatgpt_workspace_id",
                "experimental_bearer_token",
                "chatgpt_base_url",
                "cwd",
                "hooks",
                "notify",
                "session",
                "session_id",
                "shell_environment_policy",
                "features",
            ]
            .contains(&root)
            {
                return Err(format!(
                    "harnessConfig.args: --config {key} is owned by Zeroshot"
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_contained(args: &[String]) -> Result<(), String> {
    native_config::reserved_args(
        args,
        &[
            "--sandbox",
            "-s",
            "--ask-for-approval",
            "-a",
            "--full-auto",
            "--dangerously-bypass-approvals-and-sandbox",
            "--add-dir",
        ],
        native_config::ShortOptionStyle::Joined,
    )?;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let setting = if arg == "--config" || arg == "-c" {
            iter.next().map(String::as_str)
        } else {
            arg.strip_prefix("--config=")
                .or_else(|| arg.strip_prefix("-c").filter(|_| !arg.starts_with("--")))
        };
        if let Some(setting) = setting {
            let key = setting
                .trim_start_matches('=')
                .split('=')
                .next()
                .unwrap_or(setting)
                .trim();
            let root = key
                .split('.')
                .next()
                .unwrap_or(key)
                .trim_matches(['\'', '"', ' ']);
            if [
                "sandbox_mode",
                "approval_policy",
                "approvals_reviewer",
                "sandbox_workspace_write",
                "permissions",
                "default_permissions",
                "auto_review",
                "browser_use",
                "computer_use",
            ]
            .contains(&root)
            {
                return Err(format!(
                    "harnessConfig.args: --config {key} conflicts with host-owned containment"
                ));
            }
        }
    }
    Ok(())
}
