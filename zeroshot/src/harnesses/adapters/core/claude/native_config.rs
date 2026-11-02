use openengine_cluster_protocol::HarnessConfiguration;
use crate::harnesses::native_config;

pub(crate) fn validate(config: &HarnessConfiguration) -> Result<(), String> {
    let args = native_config::args(config)?;
    native_config::reserved_args(
        &args,
        &[
            "--model",
            "--provider",
            "--api-key",
            "--cwd",
            "--directory",
            "--config-dir",
            "--fallback-model",
            "--effort",
            "--agent",
            "--agents",
            "--betas",
            "--print",
            "-p",
            "--input-format",
            "--output-format",
            "--json-schema",
            "--verbose",
            "--include-partial-messages",
            "--resume",
            "-r",
            "--continue",
            "-c",
            "--session-id",
            "--from-pr",
            "--teleport",
            "--fork-session",
            "--no-session-persistence",
            "--replay-user-messages",
            "--sdk-url",
            "--permission-prompt-tool",
            "--system-prompt",
            "--system-prompt-file",
            "--append-system-prompt",
            "--append-system-prompt-file",
            "--worktree",
            "-w",
            "--bare",
            "--safe-mode",
            "--background",
            "--bg",
            "--cloud",
            "--environment",
            "--desktop",
            "--remote-control",
            "--setting-sources",
            "--settings-sources",
            "--help",
            "-h",
            "--version",
            "-v",
        ],
        native_config::ShortOptionStyle::Joined,
    )?;
    let mut iter = args.iter();
    let mut settings_seen = false;
    while let Some(arg) = iter.next() {
        let settings = if arg == "--settings" {
            iter.next().map(String::as_str)
        } else {
            arg.strip_prefix("--settings=")
        };
        if let Some(settings) = settings {
            if settings_seen {
                return Err("harnessConfig.args: use one --settings object so required hooks compose unambiguously".to_owned());
            }
            settings_seen = true;
            let object = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(settings)
                .map_err(|_| "harnessConfig.args.--settings must be an inline JSON object; settings files can shadow runtime-owned controls".to_owned())?;
            native_config::reserved_options(
                &object,
                &[
                    "env",
                    "model",
                    "modelOverrides",
                    "effortLevel",
                    "agent",
                    "agents",
                    "hooks",
                    "disableAllHooks",
                    "apiKeyHelper",
                    "awsAuthRefresh",
                    "awsCredentialExport",
                    "gcpAuthRefresh",
                    "proxyAuthHelper",
                    "forceLoginMethod",
                    "forceLoginOrgUUID",
                    "forceRemoteSettingsRefresh",
                    "otelHeadersHelper",
                ],
            )
            .map_err(|error| {
                error.replace("harnessConfig.options", "harnessConfig.args.--settings")
            })?;
        }
    }
    Ok(())
}

pub(super) fn validate_contained(args: &[String]) -> Result<(), String> {
    native_config::reserved_args(
        args,
        &[
            "--permission-mode",
            "--restricted",
            "--dangerously-skip-permissions",
            "--allow-dangerously-skip-permissions",
            "--add-dir",
        ],
        native_config::ShortOptionStyle::Joined,
    )?;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let settings = if arg == "--settings" {
            iter.next().map(String::as_str)
        } else {
            arg.strip_prefix("--settings=")
        };
        if let Some(settings) = settings.and_then(|settings| {
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(settings).ok()
        }) {
            native_config::reserved_options(
                &settings,
                &["permissions", "sandbox", "allowManagedPermissionRulesOnly"],
            )
            .map_err(|error| {
                error.replace("harnessConfig.options", "harnessConfig.args.--settings")
            })?;
        }
    }
    Ok(())
}
