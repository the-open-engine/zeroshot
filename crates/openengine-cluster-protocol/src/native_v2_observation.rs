//! Additive, run-centric observation and force-stop wire contracts for native v2.
//!
//! These types deliberately do not change the legacy Cluster Protocol v1 methods. Native-v2
//! adapters bind them to status, watch, logs, attach, and force operations while sharing one
//! public [`RunId`]. [`ExecutionRef`] remains an opaque protocol-owned selector: status pairs it
//! with a graph node name so clients can distinguish simultaneous verifier executions without
//! learning product-local execution, capsule, harness, provider, or session identities.

use std::borrow::Cow;
use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use crate::{
    AgentAttachEvent, Cursor, ExecutionRef, LogRecord, NodeName, RunConnectionRequirements, RunId,
    RunSize, RunTitle, SubscriptionId, ResolvedSource, TerminalResult, MAX_SAFE_GENERATION,
};

#[path = "native_v2_observation/failure.rs"]
mod failure;
pub use failure::*;

/// Non-negative token counter that remains exact in JavaScript clients.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct TokenCount(u64);

impl TokenCount {
    pub fn new(value: u64) -> Result<Self, TokenCountOutOfRange> {
        if value <= MAX_SAFE_GENERATION {
            Ok(Self(value))
        } else {
            Err(TokenCountOutOfRange(value))
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0
            .checked_add(other.0)
            .and_then(|value| Self::new(value).ok())
    }
}

impl<'de> Deserialize<'de> for TokenCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = crate::value::deserialize_javascript_safe_u64(deserializer, 0)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for TokenCount {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "TokenCount".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        crate::value::javascript_safe_integer_schema(0)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("token count {0} exceeds the JavaScript-safe integer maximum {MAX_SAFE_GENERATION}")]
pub struct TokenCountOutOfRange(pub u64);

/// Milliseconds since the Unix epoch for one observation.
///
/// Timestamps are positive and bounded to integers JavaScript represents exactly. Unix
/// milliseconds remain within that range for hundreds of thousands of years.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct UnixTimestampMillis(crate::PositiveInteger);

impl UnixTimestampMillis {
    /// Earliest timestamp admitted by the wire contract.
    pub const MIN: Self = Self(crate::PositiveInteger::ONE);

    pub fn new(value: u64) -> Result<Self, crate::ContractValueError> {
        crate::PositiveInteger::new(value).map(Self)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl JsonSchema for UnixTimestampMillis {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        "UnixTimestampMillis".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        crate::value::javascript_safe_integer_schema(1)
    }
}

/// Run-wide sum of provider-reported usage for every launched agent invocation.
///
/// `complete` is false when at least one invocation did not report usable counters. Cache
/// counters are omitted when the provider does not expose them consistently.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: TokenCount,
    pub output_tokens: TokenCount,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<TokenCount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<TokenCount>,
    pub complete: bool,
}

/// Runtime-owned metadata accompanying terminal status and retained run history.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<TokenUsage>,
    /// Confirmed native delivery evidence, independent of the graph's terminal output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_receipt: Option<serde_json::Value>,
}

/// One currently active graph-leaf execution.
///
/// `execution` is the stable selector used by logs and attach. `node` is the graph-visible
/// identity that makes multiple simultaneous verifier executions obvious to a client.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ActiveExecution {
    /// Zero-based map indices, outermost first; empty outside maps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub map_indices: Vec<u64>,
    pub execution: ExecutionRef,
    pub node: NodeName,
}

/// Public run state. The closed phase variants make impossible combinations unrepresentable:
/// admitted runs have no execution, stopping means force was requested, and finished runs have
/// exactly one terminal result and no active execution. Running/stopping report every active
/// execution rather than a single "current worker" slot.
#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "phase", rename_all = "snake_case")]
pub enum RunStatus {
    Admitted {},
    Running {
        #[serde(
            default,
            rename = "suspendRequested",
            skip_serializing_if = "std::ops::Not::not"
        )]
        suspend_requested: bool,
        #[serde(rename = "activeExecutions")]
        active_executions: Vec<ActiveExecution>,
        /// Recorded graph outcome while postmortem and finalization are still pending.
        /// Only `Finished.terminalResult` determines the final run outcome.
        #[serde(
            default,
            rename = "graphResult",
            skip_serializing_if = "Option::is_none"
        )]
        graph_result: Option<TerminalResult>,
    },
    Stopping {
        #[serde(rename = "activeExecutions")]
        active_executions: Vec<ActiveExecution>,
        /// Recorded graph outcome retained while stopping finalization.
        #[serde(
            default,
            rename = "graphResult",
            skip_serializing_if = "Option::is_none"
        )]
        graph_result: Option<TerminalResult>,
    },
    Suspended {
        #[serde(flatten)]
        suspension: RunSuspension,
        #[serde(default)]
        metadata: RunMetadata,
    },
    Finished {
        #[serde(rename = "terminalResult")]
        terminal_result: TerminalResult,
        #[serde(default)]
        metadata: RunMetadata,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunStatusParams {
    pub run_id: RunId,
}

/// Availability observed by a reader, separate from the run's durable status.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ControllerObservation {
    pub state: ControllerState,
    /// Time of this check, not the time the controller stopped.
    pub observed_at: UnixTimestampMillis,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerState {
    Lost,
    Unknown,
}

macro_rules! define_run_status_projection {
    (
        $(#[$attribute:meta])*
        $name:ident
        $(
            {
                run_id: #[$run_id_attribute:meta],
                title: #[$title_attribute:meta],
                source: #[$source_attribute:meta],
                size: #[$size_attribute:meta],
                at_cursor: #[$cursor_attribute:meta],
                status: #[$status_attribute:meta],
            }
        )?
    ) => {
        $(#[$attribute])*
        #[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        pub struct $name {
            $(#[$run_id_attribute])?
            pub run_id: RunId,
            $(#[$title_attribute])?
            pub title: RunTitle,
            $(#[$source_attribute])?
            #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ResolvedSource>,
            $(#[$size_attribute])?
            pub size: RunSize,
            $(#[$cursor_attribute])?
            pub at_cursor: Cursor,
            $(#[$status_attribute])?
            pub status: RunStatus,
            /// Omitted for responding controllers and finished runs. Never durable run state.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub controller: Option<ControllerObservation>,
            #[serde(default, skip_serializing_if = "WorkspaceRecovery::is_empty")]
            pub workspace_recovery: WorkspaceRecovery,
        }
    };
}

define_run_status_projection!(RunStatusResult);

/// Public recovery state. It never contains credentials, workspace paths, or checkpoint bytes.
#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct WorkspaceRecovery {
    /// Whether this run can admit a successor from retained workspace state.
    pub recoverable: bool,
    /// Exact connection fields that the successor must resolve again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub connection_requirements: RunConnectionRequirements,
    /// Immediate predecessor when this run is itself a recovery successor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<RunId>,
    /// Successor already admitted from this run, if one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_run_id: Option<RunId>,
}

impl WorkspaceRecovery {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.recoverable
            && self.connection_requirements.is_empty()
            && self.resumed_from.is_none()
            && self.successor_run_id.is_none()
    }
}

/// Establishes a durable run watch.
///
/// `fromCursor` is exclusive. Reconnecting with the last delivered cursor therefore returns each
/// later watch record once, with no replayed boundary record and no skipped later record.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunWatchParams {
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_cursor: Option<Cursor>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunWatchResult {
    pub subscription_id: SubscriptionId,
    pub run_id: RunId,
    pub at_cursor: Cursor,
}

/// One durable public status projection. `cursor` is stable run history, not a connection-local
/// sequence; clients resume strictly after it.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunWatchEventNotification {
    pub subscription_id: SubscriptionId,
    pub run_id: RunId,
    pub title: RunTitle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ResolvedSource>,
    pub size: RunSize,
    pub cursor: Cursor,
    pub status: RunStatus,
}

/// Establishes durable run log replay followed by live delivery.
///
/// An optional execution filter selects one active or settled execution using the exact opaque
/// reference advertised by status. `fromCursor` is exclusive, with the same reconnect semantics
/// as [`RunWatchParams`]. Omitting both fields after `runId` replays the run's complete retained
/// safe log history.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunLogsParams {
    pub run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_cursor: Option<Cursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionRef>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunLogsResult {
    pub subscription_id: SubscriptionId,
    pub run_id: RunId,
    pub at_cursor: Cursor,
}

/// One durable, reconnectable safe log record. Run-wide system records have no execution;
/// execution output carries the stable opaque selector used by status and attach.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunLogEventNotification {
    /// Zero-based map indices of the associated execution; empty outside maps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub map_indices: Vec<u64>,
    pub subscription_id: SubscriptionId,
    pub run_id: RunId,
    pub cursor: Cursor,
    /// Producer-captured Unix epoch milliseconds, preserved unchanged by durable replay.
    pub timestamp: UnixTimestampMillis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionRef>,
    pub record: LogRecord,
}

/// Establishes a live, read-only view of exactly one execution.
///
/// Attach has no cursor and no replay. Historical output is available through [`RunLogsParams`].
/// No client-to-execution input message exists in this contract.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunAttachParams {
    pub run_id: RunId,
    pub execution: ExecutionRef,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunAttachResult {
    pub subscription_id: SubscriptionId,
    pub run_id: RunId,
    pub execution: ExecutionRef,
}

/// One live read-only attach event. Carrying both identities prevents events from simultaneous
/// verifier attachments being confused on a multiplexed transport.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunAttachEventNotification {
    pub subscription_id: SubscriptionId,
    pub run_id: RunId,
    pub execution: ExecutionRef,
    pub event: AgentAttachEvent,
}

/// Requests the MVP's only stop mode: force. Repeated requests are idempotent at the run ledger.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunForceParams {
    pub run_id: RunId,
}

define_run_status_projection!(
    /// The durable run status after recording the force request.
    RunForceResult {
        run_id: #[doc = "Public identity of the run whose force request was recorded."],
        title: #[doc = "Immutable title captured when the run was admitted."],
        source: #[doc = "Immutable repository snapshot captured when the run was admitted."],
        size: #[doc = "Immutable execution size selected for the run."],
        at_cursor: #[doc = "Durable cursor after the force request was recorded."],
        status: #[doc = "Public phase projected after the force request was recorded."],
    }
);

/// Durable end of an attempt whose graph can continue in a successor.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "reason", rename_all = "snake_case")]
pub enum RunSuspension {
    Request {
        node: NodeName,
        #[serde(rename = "createdAt")]
        created_at: UnixTimestampMillis,
        request: Box<crate::RunRequest>,
    },
    Command {
        node: NodeName,
        #[serde(rename = "createdAt")]
        created_at: UnixTimestampMillis,
    },
    Wait {
        node: NodeName,
        #[serde(rename = "createdAt")]
        created_at: UnixTimestampMillis,
        wait: RunWait,
    },
}

impl RunSuspension {
    #[must_use]
    pub const fn node(&self) -> &NodeName {
        match self {
            Self::Command { node, .. } | Self::Wait { node, .. } | Self::Request { node, .. } => {
                node
            }
        }
    }
    #[must_use]
    pub const fn due_at(&self) -> Option<UnixTimestampMillis> {
        match self {
            Self::Command { .. } | Self::Request { .. } => None,
            Self::Wait { wait, .. } => Some(wait.due_at),
        }
    }
}

/// Absolute wall-clock deadline of a wait.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunWait {
    pub due_at: UnixTimestampMillis,
}

/// Requests a soft stop at the next checkpoint without interrupting executing workers.
pub type RunSuspendParams = RunStatusParams;
pub type RunSuspendResult = RunStatusResult;

// Deserialize the flat suspended shape explicitly: serde's flattened internally tagged enums
// cannot be combined with deny_unknown_fields. Keep strict admission for every phase.
impl<'de> Deserialize<'de> for RunStatus {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, tag = "phase", rename_all = "snake_case")]
        enum Wire {
            Admitted {},
            Running {
                #[serde(default, rename = "suspendRequested")]
                suspend_requested: bool,
                #[serde(rename = "activeExecutions")]
                active_executions: Vec<ActiveExecution>,
                #[serde(default, rename = "graphResult")]
                graph_result: Option<TerminalResult>,
            },
            Stopping {
                #[serde(rename = "activeExecutions")]
                active_executions: Vec<ActiveExecution>,
                #[serde(default, rename = "graphResult")]
                graph_result: Option<TerminalResult>,
            },
            Suspended {
                reason: crate::VectorSuspensionReason,
                node: NodeName,
                #[serde(rename = "createdAt")]
                created_at: UnixTimestampMillis,
                #[serde(default)]
                wait: Option<RunWait>,
                #[serde(default)]
                request: Option<Box<crate::RunRequest>>,
                #[serde(default)]
                metadata: RunMetadata,
            },
            Finished {
                #[serde(rename = "terminalResult")]
                terminal_result: TerminalResult,
                #[serde(default)]
                metadata: RunMetadata,
            },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Admitted {} => Self::Admitted {},
            Wire::Running {
                suspend_requested,
                active_executions,
                graph_result,
            } => Self::Running {
                suspend_requested,
                active_executions,
                graph_result,
            },
            Wire::Stopping {
                active_executions,
                graph_result,
            } => Self::Stopping {
                active_executions,
                graph_result,
            },
            Wire::Finished {
                terminal_result,
                metadata,
            } => Self::Finished {
                terminal_result,
                metadata,
            },
            Wire::Suspended {
                reason,
                node,
                created_at,
                wait,
                request,
                metadata,
            } => {
                let suspension = match (reason, wait, request) {
                    (crate::VectorSuspensionReason::Command, None, None) => {
                        RunSuspension::Command { node, created_at }
                    }
                    (crate::VectorSuspensionReason::Wait, Some(wait), None) => {
                        RunSuspension::Wait {
                            node,
                            created_at,
                            wait,
                        }
                    }
                    (crate::VectorSuspensionReason::Request, None, Some(request)) => {
                        RunSuspension::Request {
                            node,
                            created_at,
                            request,
                        }
                    }
                    _ => {
                        return Err(serde::de::Error::custom(
                            "suspension reason requires its matching wait or request member",
                        ));
                    }
                };
                Self::Suspended {
                    suspension,
                    metadata,
                }
            }
        })
    }
}
