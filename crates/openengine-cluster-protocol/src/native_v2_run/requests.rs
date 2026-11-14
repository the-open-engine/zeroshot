//! Human request contracts and the read-only vector-backed inbox.
use std::collections::BTreeMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use crate::{
    EnumLabel, FieldName, NonEmptyEnumSet, NodeName, PayloadType, RunId, UnixTimestampMillis,
    VectorId,
};

/// The question and result shape declared by a host-run request verifier.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunRequest {
    pub id: VectorId,
    pub text: String,
    pub signals: BTreeMap<FieldName, NonEmptyEnumSet>,
    pub output: PayloadType,
    pub expires_at: UnixTimestampMillis,
}

/// A person's complete verifier result; diagnostics and artifacts remain host-owned.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestAnswer {
    pub signals: BTreeMap<FieldName, EnumLabel>,
    pub output: serde_json::Value,
}

/// Resolution checked by the host before successor admission.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum RequestResolution {
    Answer { answer: RequestAnswer },
    Expiry {},
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    Pending,
    Answered,
    Expired,
    Stopped,
}

/// A bounded inbox row; text, shape and answers are available only through show.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InboxEntry {
    #[serde(deserialize_with = "super::vectors::deserialize_record_run_id")]
    #[schemars(with = "VectorId")]
    pub run_id: RunId,
    pub node: NodeName,
    pub created_at: UnixTimestampMillis,
    pub expires_at: UnixTimestampMillis,
    pub state: RequestState,
}

/// Lists requests still holding their own run suspended, in run ID order.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InboxListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<RunId>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::checkpoints::deserialize_page_limit"
    )]
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<u32>,
}
impl InboxListParams {
    pub fn validate(&self) -> Result<(), super::NativeV2RunValueError> {
        super::checkpoints::validate_page_limit(self.limit.unwrap_or(50))
    }
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InboxListResult {
    #[schemars(length(max = 100))]
    pub requests: Vec<InboxEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after: Option<RunId>,
}
pub type InboxShowParams = crate::RunStatusParams;

/// The request made by this run, including its current disposition and continuation.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InboxShowResult {
    pub run_id: RunId,
    pub node: NodeName,
    pub created_at: UnixTimestampMillis,
    pub request: RunRequest,
    pub state: RequestState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_run_id: Option<RunId>,
}
pub const INBOX_LIST_METHOD: &str = "inbox/list";
pub const INBOX_SHOW_METHOD: &str = "inbox/show";

impl From<InboxShowResult> for InboxEntry {
    fn from(request: InboxShowResult) -> Self {
        Self {
            run_id: request.run_id,
            node: request.node,
            created_at: request.created_at,
            expires_at: request.request.expires_at,
            state: request.state,
        }
    }
}

/// Full request, including node input text, authored contract and response framing.
pub const MAX_REQUEST_RESPONSE_BYTES: usize = 3 * 1024 * 1024 + 1024;
