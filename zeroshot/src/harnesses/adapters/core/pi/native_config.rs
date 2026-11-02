use openengine_cluster_protocol::HarnessConfiguration;
use crate::harnesses::native_config;

pub(crate) fn validate(config: &HarnessConfiguration) -> Result<(), String> {
    let args = native_config::args(config)?;
    // Pi recognizes whole short tokens: the native tool modifier "-read" is not "-r".
    native_config::reserved_args(
        &args,
        &[
            "--provider",
            "--model",
            "-m",
            "--models",
            "--api-key",
            "--thinking",
            "--mode",
            "--print",
            "-p",
            "--session",
            "--session-id",
            "--fork",
            "--session-dir",
            "--no-session",
            "--resume",
            "-r",
            "--continue",
            "-c",
            "--agent-dir",
            "--config",
            "--cwd",
            "--system-prompt",
            "--append-system-prompt",
            "--no-extensions",
            "-ne",
            "--help",
            "-h",
            "--version",
            "-v",
            "--export",
        ],
        native_config::ShortOptionStyle::Exact,
    )
}
