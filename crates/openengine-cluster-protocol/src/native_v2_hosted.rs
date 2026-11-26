//! Host-owned native-v2 lifecycle projections.
//!
//! A host may own a run before its OECP target exists. `queued` is the sole host-only phase;
//! every later phase is a mechanical projection of the target's OECP lifecycle.

use serde::{Deserialize, Serialize};

#[path = "native_v2_hosted/stack.rs"]
mod stack;
pub use stack::*;

#[path = "delegation.rs"]
mod delegation;
pub use delegation::*;

use crate::{
    Cursor, ResolvedSource, RunForceResult, RunId, RunListResult, RunLogEventNotification, RunSize,
    RunStatus, RunStatusResult, RunTitle, RunWatchEventNotification, SubscriptionCloseReason,
    SubscriptionId,
};

/// Lifecycle exposed by a hosted target.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum HostedRunStatus {
    Queued(HostedQueuedRunStatus),
    Target(RunStatus),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "phase", rename_all = "snake_case")]
pub enum HostedQueuedRunStatus {
    Queued {},
}

impl HostedRunStatus {
    #[must_use]
    pub const fn queued() -> Self {
        Self::Queued(HostedQueuedRunStatus::Queued {})
    }

    #[must_use]
    pub const fn target(status: RunStatus) -> Self {
        Self::Target(status)
    }

    #[must_use]
    pub const fn as_target(&self) -> Option<&RunStatus> {
        match self {
            Self::Queued(_) => None,
            Self::Target(status) => Some(status),
        }
    }
}

impl From<RunStatus> for HostedRunStatus {
    fn from(status: RunStatus) -> Self {
        Self::target(status)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HostedRunStatusResult {
    pub run_id: RunId,
    pub part_id: RunId,
    pub current_part_id: RunId,
    pub title: RunTitle,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ResolvedSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<crate::RunWorkspace>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_sources: Vec<ResolvedSource>,
    pub size: RunSize,
    pub at_cursor: Cursor,
    pub status: HostedRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller: Option<crate::ControllerObservation>,
    #[serde(default, skip_serializing_if = "crate::WorkspaceRecovery::is_empty")]
    pub workspace_recovery: crate::WorkspaceRecovery,
    #[serde(default, flatten)]
    pub steering: crate::RunSteeringStatus,
}

macro_rules! hosted_status_from {
    ($result:ty) => {
        impl From<$result> for HostedRunStatusResult {
            fn from(result: $result) -> Self {
                Self {
                    run_id: result.run_id,
                    part_id: result.part_id,
                    current_part_id: result.current_part_id,
                    title: result.title,
                    source: result.source,
                    workspace: result.workspace,
                    context_sources: result.context_sources,
                    size: result.size,
                    at_cursor: result.at_cursor,
                    status: result.status.into(),
                    controller: result.controller,
                    workspace_recovery: result.workspace_recovery,
                    steering: result.steering,
                }
            }
        }
    };
}

hosted_status_from!(RunStatusResult);
hosted_status_from!(RunForceResult);

/// Force returns the same run projection as status.
pub type HostedRunForceResult = HostedRunStatusResult;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HostedRunListResult {
    pub runs: Vec<HostedRunStatusResult>,
}

impl From<RunListResult> for HostedRunListResult {
    fn from(result: RunListResult) -> Self {
        Self {
            runs: result.runs.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HostedRunWatchEventNotification {
    /// Same ledger recording time as the target notification; omitted when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<crate::UnixTimestampMillis>,
    pub subscription_id: SubscriptionId,
    pub run_id: RunId,
    pub part_id: RunId,
    pub title: RunTitle,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ResolvedSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<crate::RunWorkspace>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_sources: Vec<ResolvedSource>,
    pub size: RunSize,
    pub cursor: Cursor,
    pub status: HostedRunStatus,
}

impl From<RunWatchEventNotification> for HostedRunWatchEventNotification {
    fn from(event: RunWatchEventNotification) -> Self {
        Self {
            recorded_at: event.recorded_at,
            subscription_id: event.subscription_id,
            run_id: event.run_id,
            part_id: event.part_id,
            title: event.title,
            source: event.source,
            workspace: event.workspace,
            context_sources: event.context_sources,
            size: event.size,
            cursor: event.cursor,
            status: event.status.into(),
        }
    }
}

/// One host lifecycle stream frame. Hosts use this shape for both watch and logs streams.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    bound(deserialize = "E: Deserialize<'de>"),
    bound(serialize = "E: Serialize"),
    deny_unknown_fields,
    rename_all = "snake_case",
    tag = "type"
)]
pub enum HostedRunStreamFrame<E> {
    Event { event: E },
    Closed { reason: SubscriptionCloseReason },
}

pub type HostedRunLogStreamFrame = HostedRunStreamFrame<RunLogEventNotification>;
pub type HostedRunWatchStreamFrame = HostedRunStreamFrame<HostedRunWatchEventNotification>;

#[cfg(test)]
#[path = "native_v2_hosted/tests.rs"]
mod tests;
