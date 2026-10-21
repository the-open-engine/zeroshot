use openengine_cluster_protocol::{
    Delegation, DelegationCreateRequest, DelegationCreated, DelegationKeyCreateRequest,
    DelegationKeyCreated, DelegationKeyList, DelegationList, DelegationOwner,
};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DelegationAction {
    List {
        owner: DelegationOwner,
        organization_id: Option<String>,
    },
    Create(DelegationCreateRequest),
    Show {
        id: String,
    },
    Disable {
        id: String,
    },
    KeyList {
        id: String,
    },
    KeyCreate {
        id: String,
        request: DelegationKeyCreateRequest,
    },
    KeyRevoke {
        id: String,
        key_id: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DelegationCommand {
    pub target: String,
    pub action: DelegationAction,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum DelegationResult {
    List(DelegationList),
    Created(DelegationCreated),
    Show(Delegation),
    KeyList(DelegationKeyList),
    KeyCreated(DelegationKeyCreated),
    Disabled { disabled: bool },
    Revoked { revoked: bool },
}
