//! Bounded server-side transport contract for selected-target run history.

use async_trait::async_trait;
use openengine_cluster_protocol::{Cursor, RunId};
use crate::native_v2_observability::history::{
    RUN_HISTORY_DEFINITION_MAX_BYTES, RUN_HISTORY_LIST_MAX_BYTES, RUN_HISTORY_PAGE_MAX_BYTES,
    RUN_HISTORY_PROBLEM_MAX_BYTES,
};

/// One of the three routes in the discovered run-history capability.
#[derive(Clone, Debug)]
pub enum RunHistoryRequest {
    List { after: Option<RunId> },
    Detail { id: RunId },
    Page { id: RunId, after: Cursor },
}

impl RunHistoryRequest {
    /// Selects one bounded list page.
    #[must_use]
    pub const fn list(after: Option<RunId>) -> Self {
        Self::List { after }
    }

    /// Selects the immutable definition for one run.
    #[must_use]
    pub const fn detail(id: RunId) -> Self {
        Self::Detail { id }
    }

    /// Selects one retained history page after the supplied cursor.
    #[must_use]
    pub const fn page(id: RunId, after: Cursor) -> Self {
        Self::Page { id, after }
    }

    /// Maximum accepted response bytes for this route.
    #[must_use]
    pub const fn maximum_response_bytes(&self) -> usize {
        match self {
            Self::List { .. } => RUN_HISTORY_LIST_MAX_BYTES,
            Self::Detail { .. } => RUN_HISTORY_DEFINITION_MAX_BYTES,
            Self::Page { .. } => RUN_HISTORY_PAGE_MAX_BYTES,
        }
    }

    /// Maximum accepted bytes for a non-success problem response.
    #[must_use]
    pub const fn maximum_problem_bytes(&self) -> usize {
        RUN_HISTORY_PROBLEM_MAX_BYTES
    }
}

/// A bounded status/body pair returned by a target history transport.
pub struct RunHistoryResponse {
    status: u16,
    body: Vec<u8>,
}

impl RunHistoryResponse {
    /// Constructs a response. The consumer separately enforces the request-specific body limit.
    pub fn new(status: u16, body: Vec<u8>) -> Result<Self, RunHistoryTransportError> {
        if !(100..=599).contains(&status) {
            return Err(malformed_response());
        }
        Ok(Self { status, body })
    }

    /// Reads one HTTP response within the request's success or problem-body bound.
    #[doc(hidden)]
    pub async fn from_http(
        request: &RunHistoryRequest,
        mut response: reqwest::Response,
    ) -> Result<Self, RunHistoryTransportError> {
        let status = response.status();
        let maximum = if status.is_success() {
            request.maximum_response_bytes()
        } else {
            request.maximum_problem_bytes()
        };
        if response
            .content_length()
            .is_some_and(|length| length > maximum as u64)
        {
            return Err(malformed_response());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| malformed_response())? {
            if body.len().saturating_add(chunk.len()) > maximum {
                return Err(malformed_response());
            }
            body.extend_from_slice(&chunk);
        }
        Self::new(status.as_u16(), body)
    }

    pub(super) fn into_parts(self) -> (u16, Vec<u8>) {
        (self.status, self.body)
    }
}

/// Opaque failure that cannot expose target coordinates or credentials to the browser.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RunHistoryTransportError {
    #[error("run history capability is incompatible")]
    Incompatible,
    #[error("run history transport is unavailable")]
    Unavailable,
}

impl RunHistoryTransportError {
    /// Creates a sanitized transport failure.
    #[must_use]
    pub const fn unavailable() -> Self {
        Self::Unavailable
    }

    /// Creates a sanitized capability/response incompatibility.
    #[must_use]
    pub const fn incompatible() -> Self {
        Self::Incompatible
    }
}

fn malformed_response() -> RunHistoryTransportError {
    RunHistoryTransportError::incompatible()
}

/// Server-side executor for one selected target. Implementations retain all authentication state.
#[async_trait]
pub trait RunHistoryTransport: Send + Sync {
    async fn get(
        &self,
        request: RunHistoryRequest,
    ) -> Result<RunHistoryResponse, RunHistoryTransportError>;
}

#[cfg(test)]
#[path = "run_history_transport/tests.rs"]
mod tests;
