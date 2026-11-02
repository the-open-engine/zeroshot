use openengine_cluster_protocol::HarnessConfiguration;
use crate::harnesses::native_config;

pub(crate) fn validate(config: &HarnessConfiguration) -> Result<(), String> {
    let options = native_config::options(config)?;
    native_config::reserved_options(
        &options,
        &[
            "model",
            "provider",
            "providers",
            "api_key",
            "base_url",
            "api_mode",
            "credentials",
            "env",
            "session_id",
            "session_db",
            "db_path",
            "cwd",
            "working_directory",
            "hermes_home",
            "reasoning",
            "reasoning_effort",
            "hooks",
            "plugins",
            "gateway",
            "platform",
            "quiet_mode",
            "stream_delta_callback",
            "tool_progress_callback",
            "response_format",
            "system_prompt",
            "prefill_messages",
            "ephemeral_system_prompt",
            "terminal",
            "code_execution",
            "auxiliary",
            "delegation",
        ],
    )
}
