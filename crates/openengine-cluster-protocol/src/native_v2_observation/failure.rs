//! Public-safe failure evidence shared by terminal results and retained logs.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{ActiveExecution, BoundedLogMessage, Cursor};

/// Matches the foreground CLI's existing recent-error summary. Full evidence stays in logs.
pub const MAX_RECORDED_FAILURES: usize = 5;

/// Metadata on an existing Error record; the record's text is the diagnostic message.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureDiagnosticKind {
    Failure,
    Incomplete,
}

/// One recorded explanation, with an optional execution selector and retained log cursor.
/// A missing cursor means there is no reference into the available durable history.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FailureDiagnostic {
    pub message: BoundedLogMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ActiveExecution>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
}

/// The known runtime cause and recent recorded node failures are deliberately separate.
/// Recorded failures are evidence; retries or parallel branches may have recovered from them.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FailureContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<FailureDiagnostic>,
    #[serde(
        default,
        serialize_with = "serialize_recorded",
        deserialize_with = "deserialize_recorded"
    )]
    #[schemars(length(max = 5))]
    pub recorded: Vec<FailureDiagnostic>,
    #[serde(default)]
    pub incomplete: bool,
}

impl FailureContext {
    pub fn record(&mut self, diagnostic: FailureDiagnostic) {
        if self.recorded.len() == MAX_RECORDED_FAILURES {
            self.recorded.remove(0);
            self.incomplete = true;
        }
        self.recorded.push(diagnostic);
    }
}

fn serialize_recorded<S>(records: &[FailureDiagnostic], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if records.len() > MAX_RECORDED_FAILURES {
        return Err(serde::ser::Error::custom(
            "too many recorded failure diagnostics",
        ));
    }
    records.serialize(serializer)
}

fn deserialize_recorded<'de, D>(deserializer: D) -> Result<Vec<FailureDiagnostic>, D::Error>
where
    D: Deserializer<'de>,
{
    let records = Vec::<FailureDiagnostic>::deserialize(deserializer)?;
    if records.len() > MAX_RECORDED_FAILURES {
        return Err(serde::de::Error::custom(
            "too many recorded failure diagnostics",
        ));
    }
    Ok(records)
}
