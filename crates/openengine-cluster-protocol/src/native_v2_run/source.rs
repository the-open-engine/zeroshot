//! Secret-free Git provenance and private transport credentials.

use std::collections::BTreeMap;
use std::fmt;

use schemars::{JsonSchema, Schema, json_schema};
use serde::{Deserialize, Serialize};

use crate::{ConnectionKey, EnvironmentVariableName, StaticConnectionValues};

use super::{
    NativeV2RunValueError, NativeV2String, NativeV2StringKind, SourceRepositoryId,
    string_kind_sealed, validate_non_control_bytes,
};

fn validate_remote(value: &str) -> Result<(), NativeV2RunValueError> {
    let invalid =
        NativeV2RunValueError("Git remote must be a credential-free HTTPS or SSH locator");
    validate_non_control_bytes(value, 2_048, invalid.0)?;
    if value.starts_with('-') || value.chars().any(char::is_whitespace) || value.contains('\\') {
        return Err(invalid);
    }
    if value.starts_with("https://") || value.starts_with("ssh://") {
        return validate_remote_url(value).map_err(|()| invalid);
    }
    if value.contains("://") {
        return Err(invalid);
    }
    validate_ssh_shorthand(value).map_err(|()| invalid)
}

fn validate_remote_url(value: &str) -> Result<(), ()> {
    validate_url_authority(value)?;
    let url = url::Url::parse(value).map_err(|_| ())?;
    if !valid_remote_host(&url)
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(());
    }
    matches!(url.scheme(), "https" | "ssh")
        .then_some(())
        .ok_or(())
}

fn valid_remote_host(url: &url::Url) -> bool {
    url.host_str().is_some_and(|host| !host.starts_with('-'))
}

fn validate_url_authority(value: &str) -> Result<(), ()> {
    let (scheme, remainder) = value.split_once("://").ok_or(())?;
    let authority = remainder.split('/').next().ok_or(())?;
    if authority.is_empty()
        || authority.starts_with('@')
        || authority.matches('@').count() > 1
        || (scheme == "https" && authority.contains('@'))
    {
        Err(())
    } else {
        Ok(())
    }
}

fn validate_ssh_shorthand(value: &str) -> Result<(), ()> {
    let delimiter = if let Some(start) = value.find('[') {
        value[start..].find("]:").map(|end| start + end + 1)
    } else {
        value.find(':')
    }
    .ok_or(())?;
    let (authority, path) = value.split_at(delimiter);
    let path = &path[1..];
    if authority.len() == 1 && authority.as_bytes()[0].is_ascii_alphabetic() {
        return Err(());
    }
    if authority.is_empty()
        || path.is_empty()
        || path.starts_with(':')
        || authority.contains('/')
        || path.contains(['?', '#'])
    {
        return Err(());
    }
    validate_remote_url(&format!("ssh://{authority}/repository"))
}

fn validate_forge_provider(value: &str) -> Result<(), NativeV2RunValueError> {
    validate_non_control_bytes(
        value,
        128,
        "source forge provider ID must be 1..=128 non-control bytes",
    )
}

native_v2_string_kind!(
    GitRemoteUrlKind,
    GitRemoteUrl,
    validate_remote,
    json_schema!({
        "type": "string",
        "minLength": 1,
        "maxLength": 2048,
        "pattern": concat!(
            r"^(?:https://(?:\[[A-Za-z0-9%:._-]+\]|[^/@:?#\\\s-][^/@:?#\\\s]*)(?::[0-9]+)?(?:/[^?#\\\s]*)?",
            r"|ssh://(?:[^/@:?#\\\s]+@)?",
            r"(?:\[[A-Za-z0-9%:._-]+\]|[^/@:?#\\\s-][^/@:?#\\\s]*)(?::[0-9]+)?(?:/[^?#\\\s]*)?",
            r"|(?:[^/@:?#\\\s]+@)?(?:\[[A-Za-z0-9%:._-]+\]|[^/@:?#\\\s-][^/@:?#\\\s]*):",
            r"(?:[^/:?#\\\s][^?#\\\s]*|/(?:[^/?#\\\s][^?#\\\s]*)?))$"
        ),
        "not": {"pattern": "^[A-Za-z]:"},
        "allOf": [
            {"pattern": r"^[^\u0000-\u001f\u007f-\u009f]+$"},
            {"if": {"pattern": "://"}, "then": {"pattern": "^(https|ssh)://"}}
        ]
    })
);
native_v2_string_kind!(
    SourceForgeProviderIdKind,
    SourceForgeProviderId,
    validate_forge_provider,
    json_schema!({
        "type": "string",
        "minLength": 1,
        "maxLength": 128,
        "pattern": r"^[^\u0000-\u001f\u007f-\u009f]+$"
    })
);

/// Optional forge identity selects one registered source-code adapter.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SourceForgeIdentity {
    pub provider: SourceForgeProviderId,
    pub repository: SourceRepositoryId,
}

/// Credential-free Git locator. Git effects use this transport; forge operations use `forge`.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResolvedGitRemote {
    pub url: GitRemoteUrl,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forge: Option<SourceForgeIdentity>,
    /// Explicit source-only connection authority; omitted for public or native local Git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<ConnectionKey>,
}

impl ResolvedGitRemote {
    /// Fields authorized for checkout and Git delivery, independently of agent node bindings.
    #[must_use]
    pub fn connection_requirements(
        &self,
    ) -> BTreeMap<ConnectionKey, std::collections::BTreeSet<EnvironmentVariableName>> {
        let Some(key) = &self.connection else {
            return BTreeMap::new();
        };
        let names = if self.url.as_str().starts_with("https://") {
            ["GIT_USERNAME", "GIT_PASSWORD"]
        } else {
            ["GIT_SSH_PRIVATE_KEY", "GIT_SSH_KNOWN_HOSTS"]
        };
        let names = names
            .into_iter()
            .filter_map(|name| EnvironmentVariableName::new(name).ok())
            .collect();
        BTreeMap::from([(key.clone(), names)])
    }
}

/// Private, bounded transport authority. It never belongs in a source record or submission digest.
#[derive(Clone, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(
    try_from = "GitTransportCredentialsWire",
    tag = "kind",
    rename_all = "snake_case"
)]
pub enum GitTransportCredentials {
    Https {
        username: String,
        password: String,
    },
    Ssh {
        #[serde(rename = "privateKey")]
        private_key: String,
        #[serde(rename = "knownHosts")]
        known_hosts: String,
    },
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
enum GitTransportCredentialsWire {
    Https {
        #[schemars(length(min = 1, max = 65_536), regex(pattern = r"^[^\u0000]+$"))]
        username: String,
        #[schemars(length(min = 1, max = 65_536), regex(pattern = r"^[^\u0000]+$"))]
        password: String,
    },
    Ssh {
        #[serde(rename = "privateKey")]
        #[schemars(length(min = 1, max = 65_536), regex(pattern = r"^[^\u0000]+$"))]
        private_key: String,
        #[serde(rename = "knownHosts")]
        #[schemars(length(min = 1, max = 65_536), regex(pattern = r"^[^\u0000]+$"))]
        known_hosts: String,
    },
}

impl TryFrom<GitTransportCredentialsWire> for GitTransportCredentials {
    type Error = NativeV2RunValueError;

    fn try_from(wire: GitTransportCredentialsWire) -> Result<Self, Self::Error> {
        let credentials = match wire {
            GitTransportCredentialsWire::Https { username, password } => {
                Self::Https { username, password }
            }
            GitTransportCredentialsWire::Ssh {
                private_key,
                known_hosts,
            } => Self::Ssh {
                private_key,
                known_hosts,
            },
        };
        credentials.connection_values()?;
        Ok(credentials)
    }
}

impl GitTransportCredentials {
    /// Exact named-connection fields consumed by the transport, with normal static-value bounds.
    pub fn connection_values(&self) -> Result<StaticConnectionValues, NativeV2RunValueError> {
        let fields = match self {
            Self::Https { username, password } => {
                [("GIT_USERNAME", username), ("GIT_PASSWORD", password)]
            }
            Self::Ssh {
                private_key,
                known_hosts,
            } => [
                ("GIT_SSH_PRIVATE_KEY", private_key),
                ("GIT_SSH_KNOWN_HOSTS", known_hosts),
            ],
        };
        let values = fields
            .into_iter()
            .map(|(name, value)| Ok((EnvironmentVariableName::new(name)?, value.clone())))
            .collect::<Result<BTreeMap<_, _>, NativeV2RunValueError>>()?;
        StaticConnectionValues::new(values)
    }

    /// Recognize a source connection without converting unrelated credentials to Git authority.
    pub fn from_connection_values(
        values: &StaticConnectionValues,
    ) -> Result<Option<Self>, NativeV2RunValueError> {
        let fields = values.as_map();
        let get = |name: &str| {
            fields
                .iter()
                .find(|(key, _)| key.as_str() == name)
                .map(|(_, value)| value.clone())
        };
        let https = (get("GIT_USERNAME"), get("GIT_PASSWORD"));
        let ssh = (get("GIT_SSH_PRIVATE_KEY"), get("GIT_SSH_KNOWN_HOSTS"));
        match (https, ssh, fields.len()) {
            ((Some(username), Some(password)), (None, None), 2) => {
                Ok(Some(Self::Https { username, password }))
            }
            ((None, None), (Some(private_key), Some(known_hosts)), 2) => Ok(Some(Self::Ssh {
                private_key,
                known_hosts,
            })),
            ((None, None), (None, None), _) => Ok(None),
            _ => Err(NativeV2RunValueError(
                "Git source connection requires exactly one complete HTTPS or SSH credential shape",
            )),
        }
    }
}

impl fmt::Debug for GitTransportCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self {
            Self::Https { .. } => "https",
            Self::Ssh { .. } => "ssh",
        };
        formatter
            .debug_struct("GitTransportCredentials")
            .field("kind", &kind)
            .field("values", &"[REDACTED]")
            .finish()
    }
}
