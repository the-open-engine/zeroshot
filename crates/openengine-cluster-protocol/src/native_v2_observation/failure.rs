//! Public-safe failure evidence shared by terminal results and retained logs.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{ActiveExecution, BoundedLogMessage, Cursor};

/// Matches the foreground CLI's existing recent-error summary. Full evidence stays in logs.
pub const MAX_RECORDED_FAILURES: usize = 5;

/// Metadata on an existing log record; its text is the public diagnostic or verifier result.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureDiagnosticKind {
    Failure,
    /// A graph-defined verifier result, without universal acceptance semantics.
    Verifier,
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

/// One completed exhausted loop visit, counted by the reducer rather than worker attempts.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExhaustedLoop {
    pub node: crate::NodeName,
    pub completed_iterations: crate::PositiveInteger,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub map_indices: Vec<u64>,
}

/// The known runtime cause and recent recorded node failures are deliberately separate.
/// Recorded failures are evidence; retries or parallel branches may have recovered from them.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FailureContext {
    /// Latest recorded result per verifier node and map path, oldest first.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "serialize_recorded",
        deserialize_with = "deserialize_recorded"
    )]
    #[schemars(length(max = 5))]
    pub verifiers: Vec<FailureDiagnostic>,
    /// Latest exhausted visit per loop node and map path.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "serialize_recorded",
        deserialize_with = "deserialize_recorded"
    )]
    #[schemars(length(max = 5))]
    pub exhausted_loops: Vec<ExhaustedLoop>,
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
    pub fn record_verifier(&mut self, diagnostic: FailureDiagnostic) {
        self.verifiers
            .retain(|prior| match (&prior.execution, &diagnostic.execution) {
                (Some(prior), Some(current)) => {
                    prior.node != current.node || prior.map_indices != current.map_indices
                }
                _ => true,
            });
        if self.verifiers.len() == MAX_RECORDED_FAILURES {
            self.verifiers.remove(0);
            self.incomplete = true;
        }
        self.verifiers.push(diagnostic);
    }

    pub fn record_exhausted_loop(&mut self, visit: ExhaustedLoop) {
        self.exhausted_loops
            .retain(|prior| prior.node != visit.node || prior.map_indices != visit.map_indices);
        if self.exhausted_loops.len() == MAX_RECORDED_FAILURES {
            self.exhausted_loops.remove(0);
            self.incomplete = true;
        }
        self.exhausted_loops.push(visit);
    }

    pub fn record(&mut self, diagnostic: FailureDiagnostic) {
        if self.recorded.len() == MAX_RECORDED_FAILURES {
            self.recorded.remove(0);
            self.incomplete = true;
        }
        self.recorded.push(diagnostic);
    }
}

fn serialize_recorded<S, T: Serialize>(records: &[T], serializer: S) -> Result<S::Ok, S::Error>
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

fn deserialize_recorded<'de, D, T: Deserialize<'de>>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
{
    let records = Vec::<T>::deserialize(deserializer)?;
    if records.len() > MAX_RECORDED_FAILURES {
        return Err(serde::de::Error::custom(
            "too many recorded failure diagnostics",
        ));
    }
    Ok(records)
}
