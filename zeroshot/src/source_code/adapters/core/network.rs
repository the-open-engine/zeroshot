//! Private caller network context, shared by local forge preflight and delivery.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use reqwest::{Certificate, ClientBuilder, NoProxy, Proxy};
use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;
use crate::source_code_provider::SourceDeliveryError;

pub(crate) const NETWORK_FIELDS: &[&str] = &[
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "all_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

#[derive(Clone, Default)]
pub(crate) struct ForgeNetwork {
    values: BTreeMap<String, String>,
}

impl std::fmt::Debug for ForgeNetwork {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForgeNetwork").finish_non_exhaustive()
    }
}

impl ForgeNetwork {
    pub(crate) fn from_snapshot(snapshot: &BTreeMap<String, String>) -> Self {
        let native = LocalHarnessEnvironment::new(snapshot.clone());
        let values = NETWORK_FIELDS
            .iter()
            .filter_map(|name| {
                snapshot
                    .get(*name)
                    .or_else(|| native.get(name))
                    .map(|value| ((*name).to_owned(), value.clone()))
            })
            .collect();
        Self { values }
    }

    /// Anchor CA overrides once in the caller snapshot, before selecting or creating a workspace.
    pub(crate) fn normalize_snapshot(
        snapshot: &mut LocalHarnessEnvironment,
        directory: &Path,
    ) -> Result<(), SourceDeliveryError> {
        if let Some(file) = snapshot.get_mut("SSL_CERT_FILE").filter(|v| !v.is_empty()) {
            *file = resolve(directory, Path::new(file))
                .to_string_lossy()
                .into_owned();
        }
        if let Some(dirs) = snapshot.get_mut("SSL_CERT_DIR").filter(|v| !v.is_empty()) {
            let paths = std::env::split_paths(dirs).map(|path| resolve(directory, &path));
            *dirs = std::env::join_paths(paths)
                .map_err(|_| SourceDeliveryError::identity("invalid SSL_CERT_DIR path list"))?
                .to_string_lossy()
                .into_owned();
        }
        Ok(())
    }

    pub(crate) fn apply_command(&self, command: &mut tokio::process::Command) {
        command.envs(&self.values);
    }

    pub(crate) fn client(
        &self,
        mut builder: ClientBuilder,
    ) -> Result<ClientBuilder, SourceDeliveryError> {
        // Never consult the detached controller's proxy environment or OS proxy configuration.
        builder = builder.no_proxy();
        let bypass = self
            .value("NO_PROXY", "no_proxy")
            .and_then(NoProxy::from_string);
        for (scheme, upper, lower) in [
            ("https", "HTTPS_PROXY", "https_proxy"),
            ("http", "HTTP_PROXY", "http_proxy"),
        ] {
            if let Some(value) = self
                .value(upper, lower)
                .or_else(|| self.value("ALL_PROXY", "all_proxy"))
            {
                let proxy = if scheme == "https" {
                    Proxy::https(value)
                } else {
                    Proxy::http(value)
                }
                .map_err(|_| {
                    SourceDeliveryError::identity(format!(
                        "invalid {upper} / ALL_PROXY network setting"
                    ))
                })?;
                builder = builder.proxy(proxy.no_proxy(bypass.clone()));
            }
        }
        if let Some(file) = self.values.get("SSL_CERT_FILE").filter(|v| !v.is_empty()) {
            builder = add_certificates(builder, Path::new(file))?;
        }
        if let Some(dirs) = self.values.get("SSL_CERT_DIR").filter(|v| !v.is_empty()) {
            for directory in std::env::split_paths(dirs) {
                let entries = std::fs::read_dir(&directory)
                    .map_err(|error| certificate_error(&directory, error))?;
                for entry in entries {
                    let path = entry
                        .map_err(|error| certificate_error(&directory, error))?
                        .path();
                    if path.is_file() {
                        builder = add_certificates(builder, &path)?;
                    }
                }
            }
        }
        Ok(builder)
    }

    /// Validate a named file even where an older gh uses only the system trust store.
    pub(crate) fn validate_certificate_file(&self) -> Result<(), SourceDeliveryError> {
        if let Some(file) = self.values.get("SSL_CERT_FILE").filter(|v| !v.is_empty()) {
            std::fs::read(file).map_err(|error| certificate_error(Path::new(file), error))?;
        }
        Ok(())
    }

    fn value(&self, upper: &str, lower: &str) -> Option<&str> {
        self.values
            .get(upper)
            .filter(|value| !value.is_empty())
            .or_else(|| self.values.get(lower).filter(|value| !value.is_empty()))
            .map(String::as_str)
    }

    pub(crate) fn secrets(&self) -> Vec<String> {
        self.values
            .iter()
            .filter(|(name, _)| name.to_ascii_uppercase().ends_with("_PROXY"))
            .flat_map(|(_, value)| proxy_secrets(value))
            .collect()
    }

    pub(crate) fn redact(&self, text: &str) -> String {
        crate::native_v2_delivery::command::redact(text, &self.secrets())
    }
}

fn resolve(directory: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        directory.join(path)
    }
}

fn add_certificates(
    mut builder: ClientBuilder,
    path: &Path,
) -> Result<ClientBuilder, SourceDeliveryError> {
    let bytes = std::fs::read(path).map_err(|error| certificate_error(path, error))?;
    let certificates =
        Certificate::from_pem_bundle(&bytes).map_err(|error| certificate_error(path, error))?;
    for certificate in certificates {
        builder = builder.add_root_certificate(certificate);
    }
    Ok(builder)
}

fn certificate_error(path: &Path, error: impl std::fmt::Display) -> SourceDeliveryError {
    SourceDeliveryError::identity(format!(
        "cannot read certificate path {}: {error}",
        path.display()
    ))
}

pub(crate) fn proxy_secrets(value: &str) -> Vec<String> {
    let mut secrets = vec![value.to_owned()];
    let url = url::Url::parse(value)
        .ok()
        .filter(|url| url.host_str().is_some())
        .or_else(|| url::Url::parse(&format!("http://{value}")).ok());
    if let Some(url) = url {
        if !url.username().is_empty() || url.password().is_some() {
            let password = url.password().unwrap_or_default();
            let username = decode_component(url.username());
            let decoded_password = decode_component(password);
            secrets.extend([
                url.username().to_owned(),
                password.to_owned(),
                crate::native_v2_delivery::git_auth::encode_user_password(url.username(), password),
                crate::native_v2_delivery::git_auth::encode_user_password(
                    &username,
                    &decoded_password,
                ),
                username,
                decoded_password,
            ]);
        }
    }
    secrets
}

fn decode_component(value: &str) -> String {
    let query = format!("value={}", value.replace('+', "%2B"));
    url::form_urlencoded::parse(query.as_bytes())
        .next()
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
