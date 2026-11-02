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
            "--list-models",
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
    )?;
    // Pi consumes one whole token for these options, including values such as "-read".
    // Unknown extension flags support --name=value; never infer an operand for a switch.
    native_config::option_args(
        &args,
        native_config::ShortOptionStyle::Exact,
        |arg| match arg {
            "--name" | "-n" | "--tools" | "-t" | "--exclude-tools" | "-xt" | "--extension"
            | "-e" | "--skill" | "--prompt-template" | "--theme" | "--use-theme" | "--tui-mode" => {
                Some(native_config::ArgumentValues::One)
            }
            _ => None,
        },
    )
}
