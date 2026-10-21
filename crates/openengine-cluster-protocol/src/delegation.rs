//! Hosted machine delegation and one-time API key responses.

use std::fmt;

use serde::{Deserialize, Serialize};

pub const API_KEY_AUTHENTICATION_KIND: &str = "zeroshot.api-key-authentication/v1";
pub const MACHINE_ORGANIZATION_HEADER: &str = "X-Zeroshot-Organization";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetApiKeyDiscovery {
    pub kind: String,
    pub header: String,
    pub scheme: String,
    pub organization_header: String,
    pub identity_path: String,
    pub delegations_path: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationOwner {
    #[default]
    User,
    Org,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DelegationKey {
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub created_at: String,
    pub expires_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Delegation {
    pub id: String,
    pub name: String,
    pub owner: DelegationOwner,
    pub user_id: Option<String>,
    pub organization_id: Option<String>,
    pub created_at: String,
    pub disabled_at: Option<String>,
    pub keys: Vec<DelegationKey>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DelegationList {
    pub delegations: Vec<Delegation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DelegationKeyList {
    pub keys: Vec<DelegationKey>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DelegationCreateRequest {
    pub name: String,
    pub owner: DelegationOwner,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization_id: Option<String>,
    pub expires_in_days: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DelegationKeyCreateRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub expires_in_days: u16,
}

/// Only creation returns the secret. Debug output always omits it.
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct DelegationCreated {
    pub delegation: Delegation,
    pub key: DelegationKey,
    pub api_key: String,
}

impl fmt::Debug for DelegationCreated {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DelegationCreated")
            .field("delegation", &self.delegation)
            .field("key", &self.key)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
pub struct DelegationKeyCreated {
    pub key: DelegationKey,
    pub api_key: String,
}

impl fmt::Debug for DelegationKeyCreated {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DelegationKeyCreated")
            .field("key", &self.key)
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}
