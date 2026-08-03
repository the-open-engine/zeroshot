use std::{collections::BTreeMap, path::Path, sync::Arc};

use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    routing::put,
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use tokio::{fs, process::Command, sync::Mutex};
use zeroize::{Zeroize, Zeroizing};

use super::backend::HostedBackend;

pub const CREDENTIAL_PORT: u16 = 8_084;
const MAX_CREDENTIAL_BYTES: usize = 4 * 1024 * 1024;
const RUNTIME_ROOT: &str = "/workspace/.zeroshot-runtime";
const REPOSITORY_ROOT: &str = "/workspace/repository";
const SETTINGS_FILE: &str = "/workspace/.zeroshot-runtime/settings.json";

#[derive(Deserialize)]
#[serde(transparent)]
struct SecretString(String);

impl SecretString {
    fn expose(&self) -> &str {
        &self.0
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RuntimeConfig {
    provider: String,
    executable: String,
    model: Option<String>,
    command: Option<SecretString>,
    setup_command: Option<SecretString>,
    environment: BTreeMap<String, SecretString>,
    files: BTreeMap<String, SecretString>,
    settings: Value,
}

impl Drop for RuntimeConfig {
    fn drop(&mut self) {
        zeroize_json(&mut self.settings);
    }
}

impl RuntimeConfig {
    fn validate(&self) -> Result<(), &'static str> {
        if !valid_identifier(&self.provider, 64) {
            return Err("runtime.provider must be a bounded provider identifier");
        }
        if !valid_identifier(&self.executable, 128) {
            return Err("runtime.executable must be a bounded executable name");
        }
        if self
            .model
            .as_ref()
            .is_some_and(|model| model.trim().is_empty() || model.len() > 512)
        {
            return Err("runtime.model must be nonempty and at most 512 bytes");
        }
        if self
            .command
            .as_ref()
            .is_some_and(|value| value.expose().trim().is_empty() || value.expose().len() > 4_096)
        {
            return Err("runtime.command must be nonempty and at most 4096 bytes");
        }
        if self.setup_command.as_ref().is_some_and(|value| {
            value.expose().trim().is_empty() || value.expose().len() > 16 * 1_024
        }) {
            return Err("runtime.setupCommand must be nonempty and at most 16384 bytes");
        }
        if self.environment.len() > 256
            || self.environment.iter().any(|(name, value)| {
                !valid_environment_name(name) || value.expose().len() > 64 * 1_024
            })
        {
            return Err("runtime.environment exceeds its name, count, or value bounds");
        }
        if self.files.len() > 128
            || self.files.iter().any(|(name, value)| {
                !valid_runtime_path(name) || value.expose().len() > 512 * 1_024
            })
        {
            return Err("runtime.files exceeds its path, count, or value bounds");
        }
        if !self.settings.is_object() {
            return Err("runtime.settings must be an object");
        }
        Ok(())
    }
}

fn zeroize_json(value: &mut Value) {
    match value {
        Value::String(item) => item.zeroize(),
        Value::Array(items) => items.iter_mut().for_each(zeroize_json),
        Value::Object(items) => items.values_mut().for_each(zeroize_json),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CredentialBundle {
    github_token: SecretString,
    repository: String,
    runtime: RuntimeConfig,
}

impl CredentialBundle {
    fn validate(&self) -> Result<(), &'static str> {
        if self.github_token.expose().trim().is_empty() || self.github_token.expose().len() > 4_096
        {
            return Err("githubToken must be nonempty and at most 4096 bytes");
        }
        if !valid_repository(&self.repository) {
            return Err("repository must have the form owner/name");
        }
        self.runtime.validate()
    }

    fn apply_common_to(&self, command: &mut Command) {
        let inherited_path =
            std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".to_owned());
        command
            .env("HOME", RUNTIME_ROOT)
            .env("TMPDIR", format!("{RUNTIME_ROOT}/tmp"))
            .env(
                "PATH",
                format!("{RUNTIME_ROOT}/.local/bin:{inherited_path}:{RUNTIME_ROOT}/bin"),
            )
            .env("ZEROSHOT_SETTINGS_FILE", SETTINGS_FILE)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", format!("{RUNTIME_ROOT}/git-askpass.sh"));
    }

    fn apply_source_to(&self, command: &mut Command) {
        command.env_clear();
        self.apply_common_to(command);
        command
            .env("GH_TOKEN", self.github_token.expose())
            .env("GITHUB_TOKEN", self.github_token.expose());
    }

    pub fn apply_worker_to(&self, command: &mut Command) {
        command.env_clear();
        for (name, value) in &self.runtime.environment {
            command.env(name, value.expose());
        }
        self.apply_common_to(command);
        command
            .env("GH_TOKEN", self.github_token.expose())
            .env("GITHUB_TOKEN", self.github_token.expose())
            .env("ZEROSHOT_HOSTED_PROVIDER", &self.runtime.provider);
        if let Some(model) = &self.runtime.model {
            command.env("ZEROSHOT_HOSTED_MODEL", model);
        }
    }

    fn apply_setup_to(&self, command: &mut Command) {
        command.env_clear();
        for (name, value) in &self.runtime.environment {
            command.env(name, value.expose());
        }
        self.apply_common_to(command);
    }

    pub async fn prepare_workspace(&self) -> Result<(), String> {
        write_runtime_files(&self.runtime).await?;
        if let Some(setup_command) = &self.runtime.setup_command {
            let mut command = Command::new("sh");
            command.args(["-c", setup_command.expose()]);
            self.apply_setup_to(&mut command);
            run(&mut command, "runtime setup").await?;
        }
        if !Path::new(&format!("{REPOSITORY_ROOT}/.git")).exists() {
            if Path::new(REPOSITORY_ROOT).exists() {
                fs::remove_dir_all(REPOSITORY_ROOT)
                    .await
                    .map_err(|error| format!("remove incomplete repository clone: {error}"))?;
            }
            fs::create_dir_all("/workspace")
                .await
                .map_err(|error| format!("create workspace: {error}"))?;
            let mut command = Command::new("git");
            command.args([
                "clone",
                "--filter=blob:none",
                &format!("https://github.com/{}.git", self.repository),
                REPOSITORY_ROOT,
            ]);
            self.apply_source_to(&mut command);
            run(&mut command, "git clone").await?;
        }
        configure_git_identity(self).await
    }
}

async fn configure_git_identity(credentials: &CredentialBundle) -> Result<(), String> {
    let mut identity_command = Command::new("gh");
    identity_command.args(["api", "user", "--jq", "[.login, (.id|tostring)] | @tsv"]);
    credentials.apply_source_to(&mut identity_command);
    let output = run(&mut identity_command, "GitHub identity lookup").await?;
    let (name, email) = github_identity(&String::from_utf8_lossy(&output))?;

    for (key, value) in [("user.name", name.as_str()), ("user.email", email.as_str())] {
        let mut command = Command::new("git");
        command.args(["-C", REPOSITORY_ROOT, "config", "--local", key, value]);
        credentials.apply_source_to(&mut command);
        run(&mut command, "git identity configuration").await?;
    }
    Ok(())
}

fn github_identity(value: &str) -> Result<(String, String), String> {
    let (login, id) = value
        .trim()
        .split_once('\t')
        .ok_or_else(|| "GitHub identity lookup returned an invalid response".to_owned())?;
    if login.is_empty()
        || !login
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || id.is_empty()
        || !id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("GitHub identity lookup returned an invalid response".to_owned());
    }
    Ok((
        login.to_owned(),
        format!("{id}+{login}@users.noreply.github.com"),
    ))
}

async fn run(command: &mut Command, operation: &str) -> Result<Vec<u8>, String> {
    let output = command
        .output()
        .await
        .map_err(|error| format!("start {operation}: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "{operation} failed with status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn valid_repository(value: &str) -> bool {
    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    !owner.is_empty()
        && !name.is_empty()
        && !matches!(owner, "." | "..")
        && !matches!(name, "." | "..")
        && parts.next().is_none()
        && owner.bytes().all(valid_repo_byte)
        && name.bytes().all(valid_repo_byte)
}

fn valid_identifier(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_environment_name(value: &str) -> bool {
    value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn valid_runtime_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.starts_with('/')
        && !value.contains('\\')
        && value
            .split('/')
            .all(|segment| !segment.is_empty() && !matches!(segment, "." | ".."))
}

fn valid_repo_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
}

async fn write_runtime_files(runtime: &RuntimeConfig) -> Result<(), String> {
    fs::create_dir_all(RUNTIME_ROOT)
        .await
        .map_err(|error| format!("create runtime directory: {error}"))?;
    for directory in ["tmp", "bin", ".local/bin"] {
        fs::create_dir_all(format!("{RUNTIME_ROOT}/{directory}"))
            .await
            .map_err(|error| format!("create runtime {directory} directory: {error}"))?;
    }

    let settings = Zeroizing::new(
        serde_json::to_vec(&runtime.settings)
            .map_err(|error| format!("serialize runtime settings: {error}"))?,
    );
    fs::write(SETTINGS_FILE, &*settings)
        .await
        .map_err(|error| format!("write runtime settings: {error}"))?;

    for (filename, contents) in &runtime.files {
        let destination = format!("{RUNTIME_ROOT}/{filename}");
        if let Some(parent) = Path::new(&destination).parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|error| format!("create runtime file parent: {error}"))?;
        }
        fs::write(&destination, contents.expose())
            .await
            .map_err(|error| format!("write runtime file {filename}: {error}"))?;
        protect(&destination, 0o600).await?;
    }

    if let Some(provider_command) = &runtime.command {
        let wrapper = format!("{RUNTIME_ROOT}/bin/{}", runtime.executable);
        fs::write(
            &wrapper,
            format!("#!/bin/sh\nexec {} \"$@\"\n", provider_command.expose()),
        )
        .await
        .map_err(|error| format!("write runtime command wrapper: {error}"))?;
        protect(&wrapper, 0o700).await?;
    }

    fs::write(
        format!("{RUNTIME_ROOT}/git-askpass.sh"),
        "#!/bin/sh\ncase \"$1\" in\n  *Username*) printf '%s\\n' x-access-token ;;\n  *) printf '%s\\n' \"$GH_TOKEN\" ;;\nesac\n",
    )
    .await
    .map_err(|error| format!("write git credential helper: {error}"))?;
    protect(SETTINGS_FILE, 0o600).await?;
    protect(&format!("{RUNTIME_ROOT}/git-askpass.sh"), 0o700).await?;
    for directory in [RUNTIME_ROOT, "tmp", "bin", ".local", ".local/bin"] {
        let path = if directory == RUNTIME_ROOT {
            RUNTIME_ROOT.to_owned()
        } else {
            format!("{RUNTIME_ROOT}/{directory}")
        };
        protect(&path, 0o700).await?;
    }
    Ok(())
}

#[cfg(unix)]
async fn protect(path: &str, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .await
        .map_err(|error| format!("protect runtime path {path}: {error}"))
}

#[cfg(not(unix))]
async fn protect(_path: &str, _mode: u32) -> Result<(), String> {
    Ok(())
}

pub fn router(backend: Arc<HostedBackend>) -> Router {
    Router::new()
        .route("/internal/credentials", put(install))
        .layer(DefaultBodyLimit::max(MAX_CREDENTIAL_BYTES))
        .with_state(backend)
}

async fn install(
    State(backend): State<Arc<HostedBackend>>,
    Json(bundle): Json<CredentialBundle>,
) -> Result<StatusCode, (StatusCode, &'static str)> {
    bundle
        .validate()
        .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
    backend
        .install_credentials(bundle)
        .await
        .map_err(|message| (StatusCode::CONFLICT, message))?;
    Ok(StatusCode::NO_CONTENT)
}

pub type CredentialSlot = Arc<Mutex<Option<CredentialBundle>>>;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::{github_identity, CredentialBundle, RuntimeConfig, SecretString, RUNTIME_ROOT};
    use crate::hosted_oecp::HostedBackend;

    fn secret(value: &str) -> SecretString {
        SecretString(value.to_owned())
    }

    fn bundle() -> CredentialBundle {
        CredentialBundle {
            github_token: secret("github"),
            repository: "the-open-engine/zeroshot".to_owned(),
            runtime: RuntimeConfig {
                provider: "gemini".to_owned(),
                executable: "gemini".to_owned(),
                model: Some("gemini-2.5-pro".to_owned()),
                command: Some(secret("npx --yes @google/gemini-cli")),
                setup_command: Some(secret("npm --version")),
                environment: BTreeMap::from([
                    (
                        "CUSTOM_ENDPOINT".to_owned(),
                        secret("https://models.example"),
                    ),
                    ("GOOGLE_API_KEY".to_owned(), secret("provider-secret")),
                    ("HOME".to_owned(), secret("/untrusted-home")),
                ]),
                files: BTreeMap::from([(
                    ".config/harness/config.json".to_owned(),
                    secret("{\"endpoint\":\"https://models.example\"}"),
                )]),
                settings: json!({
                    "defaultProvider": "gemini",
                    "providerSettings": {"gemini": {"defaultLevel": "level2"}}
                }),
            },
        }
    }

    fn command_environment(command: &tokio::process::Command) -> BTreeMap<String, Option<String>> {
        command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                Some((
                    key.to_str()?.to_owned(),
                    value.and_then(|item| item.to_str()).map(str::to_owned),
                ))
            })
            .collect()
    }

    #[test]
    fn credential_contract_is_closed_and_bounded() {
        assert!(bundle().validate().is_ok());
        let mut invalid = bundle();
        invalid.repository = "owner/repo/extra".to_owned();
        assert!(invalid.validate().is_err());
        let mut invalid = bundle();
        invalid
            .runtime
            .environment
            .insert("INVALID-NAME".to_owned(), secret("value"));
        assert!(invalid.validate().is_err());
        let mut invalid = bundle();
        invalid
            .runtime
            .files
            .insert("../escape".to_owned(), secret("value"));
        assert!(invalid.validate().is_err());
        assert!(
            serde_json::from_value::<CredentialBundle>(json!({
                "githubToken": "github",
                "repository": "the-open-engine/zeroshot",
                "runtime": {
                    "provider": "future-provider",
                    "executable": "future-cli",
                    "environment": {},
                    "files": {},
                    "settings": {},
                    "extra": true
                }
            }))
            .is_err()
        );
    }

    #[test]
    fn source_environment_keeps_only_git_auth_and_runtime_control() {
        let mut command = tokio::process::Command::new("true");
        bundle().apply_source_to(&mut command);
        let environment = command_environment(&command);
        assert_eq!(
            environment.get("GIT_ASKPASS"),
            Some(&Some(format!("{RUNTIME_ROOT}/git-askpass.sh")))
        );
        assert_eq!(
            environment.get("GIT_TERMINAL_PROMPT"),
            Some(&Some("0".to_owned()))
        );
        assert_eq!(
            environment.get("GH_TOKEN"),
            Some(&Some("github".to_owned()))
        );
        assert_eq!(environment.get("GOOGLE_API_KEY"), None);
        assert_eq!(environment.get("CUSTOM_ENDPOINT"), None);
    }

    #[test]
    fn worker_receives_the_cli_owned_generic_runtime() {
        let mut command = tokio::process::Command::new("true");
        bundle().apply_worker_to(&mut command);
        let environment = command_environment(&command);

        assert_eq!(
            environment.get("ZEROSHOT_HOSTED_PROVIDER"),
            Some(&Some("gemini".to_owned()))
        );
        assert_eq!(
            environment.get("ZEROSHOT_HOSTED_MODEL"),
            Some(&Some("gemini-2.5-pro".to_owned()))
        );
        assert_eq!(
            environment.get("GOOGLE_API_KEY"),
            Some(&Some("provider-secret".to_owned()))
        );
        assert_eq!(
            environment.get("CUSTOM_ENDPOINT"),
            Some(&Some("https://models.example".to_owned()))
        );
        assert_eq!(
            environment.get("HOME"),
            Some(&Some(RUNTIME_ROOT.to_owned()))
        );
    }

    #[test]
    fn github_identity_uses_the_authenticated_accounts_noreply_address() {
        assert_eq!(
            github_identity("zeroshot-user\t12345\n"),
            Ok((
                "zeroshot-user".to_owned(),
                "12345+zeroshot-user@users.noreply.github.com".to_owned()
            ))
        );
        assert!(github_identity("bad login\t12345").is_err());
        assert!(github_identity("zeroshot-user\tnot-an-id").is_err());
    }

    #[tokio::test]
    async fn credentials_can_only_be_installed_once() {
        let backend = HostedBackend::new();
        assert!(backend.install_credentials(bundle()).await.is_ok());
        assert_eq!(
            backend.install_credentials(bundle()).await,
            Err("credentials are already installed")
        );
    }
}
