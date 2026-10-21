use openengine_cluster_protocol::{
    API_KEY_AUTHENTICATION_KIND, MACHINE_ORGANIZATION_HEADER, TargetDiscoveryDocument,
};
use reqwest::{Url, header::HeaderValue};

use super::TargetHttpControlAuthority;
use super::contract::{authority_error, same_origin_path, validate_secret};
use crate::native_v2_target::TargetAuthorityError;

#[derive(serde::Deserialize)]
struct MachineIdentity {
    owner: openengine_cluster_protocol::DelegationOwner,
}

#[path = "machine/delegations.rs"]
mod delegations;

/// Target credentials stay in memory and only enter the target Authorization header.
#[derive(Clone)]
pub(super) struct MachineCredential {
    pub(super) api_key: String,
    pub(super) organization: Option<HeaderValue>,
}

impl std::fmt::Debug for MachineCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MachineCredential")
            .field("api_key", &"[REDACTED]")
            .field("organization", &self.organization)
            .finish()
    }
}

impl MachineCredential {
    pub(super) fn from_environment() -> Result<Option<Self>, TargetAuthorityError> {
        Self::from_environment_with(|name| std::env::var_os(name))
    }

    fn from_environment_with(
        environment: impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Result<Option<Self>, TargetAuthorityError> {
        let Some(value) = environment("ZEROSHOT_API_KEY") else {
            return Ok(None);
        };
        let api_key = value
            .into_string()
            .map_err(|_| authority_error("ZEROSHOT_API_KEY must be valid UTF-8"))?;
        validate_secret(&api_key, "ZEROSHOT_API_KEY")?;
        if api_key.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return Err(authority_error("ZEROSHOT_API_KEY is malformed"));
        }
        let organization = environment("ZEROSHOT_ORGANIZATION")
            .map(|value| {
                let value = value.into_string().map_err(|_| {
                    authority_error("ZEROSHOT_ORGANIZATION must be an organization UUID")
                })?;
                if uuid::Uuid::parse_str(&value).is_err() {
                    return Err(authority_error(
                        "ZEROSHOT_ORGANIZATION must be an organization UUID",
                    ));
                }
                HeaderValue::from_str(&value)
                    .map_err(|_| authority_error("ZEROSHOT_ORGANIZATION is malformed"))
            })
            .transpose()?;
        Ok(Some(Self {
            api_key,
            organization,
        }))
    }
}

#[cfg(test)]
#[path = "machine/tests.rs"]
mod tests;

impl TargetHttpControlAuthority {
    pub(super) async fn machine_default_scope(
        &self,
        target: &crate::native_v2_target::TargetRecord,
    ) -> Result<openengine_cluster_protocol::ConnectionScope, TargetAuthorityError> {
        use openengine_cluster_protocol::{ConnectionScope, DelegationOwner};
        let Some(machine) = &self.machine else {
            return Ok(ConnectionScope::User);
        };
        let identity = self.machine_identity(target, machine).await?;
        Ok(match identity.owner {
            DelegationOwner::User => ConnectionScope::User,
            DelegationOwner::Org => ConnectionScope::Org,
        })
    }

    async fn machine_identity(
        &self,
        target: &crate::native_v2_target::TargetRecord,
        machine: &MachineCredential,
    ) -> Result<MachineIdentity, TargetAuthorityError> {
        if matches!(target.access, crate::native_v2_target::TargetAccess::Direct) {
            return Err(authority_error("ZEROSHOT_API_KEY requires a hosted target"));
        }
        let (origin, wire) = self.discovery(target).await?;
        validate_discovery(&origin, &wire)?;
        let capability =
            wire.extensions.api_key.as_ref().ok_or_else(|| {
                authority_error("target does not advertise API key authentication")
            })?;
        let url = same_origin_path(&origin, &capability.identity_path)?;
        let response = self
            .authorized(self.client.get(url.clone()), &machine.api_key)?
            .header(reqwest::header::CACHE_CONTROL, "no-store")
            .send()
            .await
            .map_err(|error| TargetAuthorityError::request_failed("machine identity", &error))?;
        super::contract::read_success_json(response, &url, "machine identity").await
    }

    pub(super) fn require_human_credential(&self) -> Result<(), TargetAuthorityError> {
        if self.machine.is_some() {
            return Err(authority_error(
                "human login required; unset ZEROSHOT_API_KEY for delegation management or target login",
            ));
        }
        Ok(())
    }

    pub(super) fn require_no_machine_for_direct(&self) -> Result<(), TargetAuthorityError> {
        if self.machine.is_some() {
            return Err(authority_error("ZEROSHOT_API_KEY requires a hosted target"));
        }
        Ok(())
    }
}

pub(super) fn validate_discovery(
    origin: &Url,
    wire: &TargetDiscoveryDocument,
) -> Result<(), TargetAuthorityError> {
    let capability = wire
        .extensions
        .api_key
        .as_ref()
        .ok_or_else(|| authority_error("target does not advertise API key authentication"))?;
    if capability.kind != API_KEY_AUTHENTICATION_KIND
        || capability.header != "Authorization"
        || capability.scheme != "Bearer"
        || capability.organization_header != MACHINE_ORGANIZATION_HEADER
    {
        return Err(authority_error(
            "API key authentication discovery is incompatible",
        ));
    }
    same_origin_path(origin, &capability.identity_path)?;
    same_origin_path(origin, &capability.delegations_path)?;
    Ok(())
}
