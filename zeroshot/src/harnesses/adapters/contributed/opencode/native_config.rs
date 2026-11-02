use openengine_cluster_protocol::HarnessConfiguration;
use crate::harnesses::native_config;

pub(crate) fn validate(config: &HarnessConfiguration) -> Result<(), String> {
    let args = config.args().unwrap_or_default();
    if let Some(options) = config.options() {
        native_config::reserved_options(
            options,
            &[
                "model",
                "small_model",
                "provider",
                "disabled_providers",
                "enabled_providers",
                "agent",
                "mode",
                "plugin",
                "instructions",
                "server",
                "username",
                "password",
                "command",
                "mcp",
                "experimental",
            ],
        )?;
    }
    native_config::reserved_args(
        &args,
        &[
            "--model",
            "--provider",
            "--api-key",
            "-m",
            "--variant",
            "--agent",
            "--format",
            "--session",
            "-s",
            "--continue",
            "-c",
            "--fork",
            "--dir",
            "--cwd",
            "--attach",
            "--port",
            "--hostname",
            "--password",
            "--username",
            "--file",
            "-f",
            "--prompt",
            "--command",
            "--config",
            "--help",
            "-h",
            "--version",
            "-v",
        ],
        native_config::ShortOptionStyle::Joined,
    )
}
