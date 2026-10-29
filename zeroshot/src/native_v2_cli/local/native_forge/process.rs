use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;

use super::{NativeContext, NativeV2CliError, GITHUB, GITLAB, local_message};

const NATIVE_CONTEXT_FIELDS: &[&str] = &[
    "HOME",
    "PATH",
    "GH_CONFIG_DIR",
    "GLAB_CONFIG_DIR",
    "XDG_CONFIG_HOME",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "APPDATA",
    "LOCALAPPDATA",
    "USERPROFILE",
    "SystemRoot",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "all_proxy",
    "no_proxy",
    "LANG",
    "LC_ALL",
];

pub(super) fn environment_token(
    provider: &str,
    hostname: &str,
    environment: &BTreeMap<String, String>,
) -> Result<Option<String>, NativeV2CliError> {
    let snapshot = LocalHarnessEnvironment::new(environment.clone());
    environment_token_from_snapshot(provider, hostname, &snapshot)
}

fn environment_token_from_snapshot(
    provider: &str,
    hostname: &str,
    environment: &LocalHarnessEnvironment,
) -> Result<Option<String>, NativeV2CliError> {
    let (selector, default_host, names): (&str, &str, &[&str]) = match provider {
        GITHUB if hostname == "github.com" || hostname.ends_with(".ghe.com") => {
            ("GH_HOST", "github.com", &["GH_TOKEN", "GITHUB_TOKEN"])
        }
        GITHUB => (
            "GH_HOST",
            "github.com",
            &["GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"],
        ),
        GITLAB => (
            "GITLAB_HOST",
            "gitlab.com",
            &["GITLAB_TOKEN", "GITLAB_ACCESS_TOKEN", "OAUTH_TOKEN"],
        ),
        _ => return Ok(None),
    };
    let selected = environment
        .get(selector)
        .map_or(default_host, String::as_str);
    if environment_hostname(selected).as_deref() != Some(hostname) {
        return Ok(None);
    }
    names
        .iter()
        .find_map(|name| environment.get(name))
        .map(|value| checked_token(value).map(str::to_owned))
        .transpose()
}

// Caller input can choose a destination, but cannot choose the authority of an ambient token.
fn environment_hostname(value: &str) -> Option<String> {
    if value.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return None;
    }
    let address = if value.contains("://") {
        value.to_owned()
    } else {
        format!("https://{value}")
    };
    let parsed = url::Url::parse(&address).ok()?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    let host = parsed.host()?;
    Some(
        parsed
            .port()
            .map_or_else(|| host.to_string(), |port| format!("{host}:{port}")),
    )
}

pub(super) async fn stored_token(
    provider: &str,
    hostname: &str,
    context: NativeContext<'_>,
) -> Result<Option<String>, NativeV2CliError> {
    let executable = match provider {
        GITHUB => "gh",
        GITLAB => "glab",
        _ => return Ok(None),
    };
    let Some(mut command) = native_command(executable, context) else {
        return Ok(None);
    };
    let output = capture_native(&mut command, provider, hostname).await?;
    decode_native_output(provider, hostname, &output)
}

fn decode_native_output(
    provider: &str,
    hostname: &str,
    output: &crate::native_v2_delivery::command::GitCommandOutput,
) -> Result<Option<String>, NativeV2CliError> {
    if output.diagnostic().exit_status != Some(0) {
        return Ok(None);
    }
    if output.stdout_truncated() || output.stdout().len() > 16 * 1024 {
        return Err(local_message(
            "native forge credential response exceeded its bound",
        ));
    }
    let token = if provider == GITHUB {
        output.stdout().trim().to_owned()
    } else {
        git_credential_token(output.stdout(), hostname)?
    };
    if token.is_empty() {
        return Ok(None);
    }
    checked_token(&token).map(|token| Some(token.to_owned()))
}

fn native_command(executable: &str, context: NativeContext<'_>) -> Option<Command> {
    let program = crate::execution::platform::find_executable(
        executable,
        context.environment,
        context.directory,
    )?;
    let mut command = Command::new(program);
    command
        .env_clear()
        .current_dir(context.directory)
        .stdin(Stdio::null());
    for field in NATIVE_CONTEXT_FIELDS {
        if let Some(value) = context.environment.get(*field) {
            command.env(field, value);
        }
    }
    command
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GLAB_CHECK_UPDATE", "false")
        .env("GIT_TERMINAL_PROMPT", "0");
    Some(command)
}

async fn capture_native(
    command: &mut Command,
    provider: &str,
    hostname: &str,
) -> Result<crate::native_v2_delivery::command::GitCommandOutput, NativeV2CliError> {
    let output = if provider == GITHUB {
        command.args(["auth", "token", "--hostname", hostname]);
        crate::native_v2_delivery::command::capture_sensitive(command, Duration::from_secs(10), &[])
            .await
    } else {
        command.args(["auth", "git-credential", "get"]);
        let input = format!("protocol=https\nhost={hostname}\n\n");
        crate::native_v2_delivery::command::capture_sensitive_with_input(
            command,
            Duration::from_secs(10),
            &[],
            input.as_bytes(),
        )
        .await
    };
    output.map_err(|_| local_message("native forge credential lookup failed"))
}

fn checked_token(value: &str) -> Result<&str, NativeV2CliError> {
    if value.is_empty() || value.len() > 4096 || !value.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(local_message("native forge credential is invalid"));
    }
    Ok(value)
}

fn git_credential_token(output: &str, hostname: &str) -> Result<String, NativeV2CliError> {
    let mut password = None;
    for line in output.lines().filter(|line| !line.is_empty()) {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| local_message("native Git credential response is malformed"))?;
        if key == "password" {
            if password.replace(value.to_owned()).is_some() {
                return Err(local_message("native Git credential response is ambiguous"));
            }
        } else {
            validate_git_field(key, value, hostname)?;
        }
    }
    Ok(password.unwrap_or_default())
}

fn validate_git_field(key: &str, value: &str, hostname: &str) -> Result<(), NativeV2CliError> {
    match key {
        "host" if value != hostname => Err(local_message(
            "native Git credential response changed its host",
        )),
        "protocol" if value != "https" => Err(local_message(
            "native Git credential response changed its protocol",
        )),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "process/tests.rs"]
mod tests;
