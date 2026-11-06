use std::error::Error as _;

use thiserror::Error;
use openengine_cluster_protocol::IdempotencyKey;
use zeroshot_engine::native_v2_cli::NativeV2CliError;

use super::TargetRecord;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct TargetAuthorityError {
    message: String,
    transport: Option<TransportFailure>,
    remote: Option<TargetRemoteError>,
}

/// A request that failed without a target response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransportFailure {
    /// The request never reached the target.
    Unsent,
    /// The request may have reached the target before the connection failed.
    Unconfirmed,
    /// The target may have admitted a run submission whose receipt was lost.
    UnconfirmedSubmission,
}

#[derive(Debug)]
struct TargetRemoteError {
    code: String,
    details: Option<serde_json::Value>,
}

impl TargetAuthorityError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            transport: None,
            remote: None,
        }
    }

    pub(super) fn disconnected(message: impl Into<String>) -> Self {
        Self::transport(message, TransportFailure::Unsent)
    }

    fn transport(message: impl Into<String>, failure: TransportFailure) -> Self {
        Self {
            message: message.into(),
            transport: Some(failure),
            remote: None,
        }
    }

    pub(super) fn request_failed(operation: &str, error: &reqwest::Error) -> Self {
        let refused = std::iter::successors(error.source(), |&source| source.source())
            .filter_map(|source| source.downcast_ref::<std::io::Error>())
            .any(|error| error.kind() == std::io::ErrorKind::ConnectionRefused);
        // Raw HTTP errors can contain request URLs and credentials. Retain only a safe category.
        let reason = if error.is_timeout() {
            "request timed out"
        } else if refused {
            "connection refused"
        } else if error.is_connect() {
            "could not establish a connection (check network, DNS and TLS settings)"
        } else {
            "connection interrupted"
        };
        // A connection that was never established, or a request that was never built, cannot
        // have reached the target. Any later failure leaves its effect unconfirmed.
        let failure = if refused || error.is_connect() || error.is_builder() {
            TransportFailure::Unsent
        } else {
            TransportFailure::Unconfirmed
        };
        Self::transport(format!("{operation} failed: {reason}"), failure)
    }

    /// Marks a failure of the request that submits a run. If that request may have reached the
    /// target, the target may have admitted the run without its receipt reaching the caller.
    #[must_use]
    pub(super) fn submission(mut self) -> Self {
        if self.transport == Some(TransportFailure::Unconfirmed) {
            self.transport = Some(TransportFailure::UnconfirmedSubmission);
        }
        self
    }

    pub(super) fn is_unconfirmed_submission(&self) -> bool {
        self.transport == Some(TransportFailure::UnconfirmedSubmission)
    }

    pub(super) fn is_transport_failure(&self) -> bool {
        self.transport.is_some()
    }

    pub(super) fn remote(
        code: impl Into<String>,
        message: impl Into<String>,
        details: Option<serde_json::Value>,
    ) -> Self {
        Self {
            message: message.into(),
            transport: None,
            remote: Some(TargetRemoteError {
                code: code.into(),
                details,
            }),
        }
    }

    pub(super) fn redact_secret(mut self, secret: &str) -> Self {
        if secret.is_empty() {
            return self;
        }
        self.message = self.message.replace(secret, "[REDACTED]");
        if let Some(remote) = &mut self.remote {
            remote.code = remote.code.replace(secret, "redacted");
            if let Some(details) = &mut remote.details {
                redact_json(details, secret);
            }
        }
        self
    }

    pub(super) fn is_http_auth_rejection(&self) -> bool {
        self.remote
            .as_ref()
            .and_then(|remote| remote.details.as_ref())
            .and_then(|details| details.get("httpStatus"))
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|status| matches!(status, 401 | 403))
    }

    pub(super) fn into_cli(self, target: &TargetRecord) -> NativeV2CliError {
        if self.transport.is_some() {
            NativeV2CliError::TargetTransport {
                name: target.name.clone(),
                origin: target.origin.clone(),
                message: self.message,
            }
        } else if let Some(remote) = self.remote {
            NativeV2CliError::Remote {
                code: remote.code,
                message: self.message,
                details: remote.details,
            }
        } else {
            NativeV2CliError::Target(self.message)
        }
    }

    pub(super) fn into_unconfirmed_submission(
        self,
        target: &TargetRecord,
        submission_key: &IdempotencyKey,
    ) -> NativeV2CliError {
        NativeV2CliError::SubmissionOutcomeUnknown {
            name: target.name.clone(),
            origin: target.origin.clone(),
            submission_key: submission_key.as_str().to_owned(),
            message: self.message,
        }
    }
}

pub(super) fn redact_json(value: &mut serde_json::Value, secret: &str) {
    if secret.is_empty() {
        return;
    }
    match value {
        serde_json::Value::String(text) => *text = text.replace(secret, "[REDACTED]"),
        serde_json::Value::Array(values) => {
            for value in values {
                redact_json(value, secret);
            }
        }
        serde_json::Value::Object(fields) => {
            *fields = std::mem::take(fields)
                .into_iter()
                .map(|(name, mut value)| {
                    redact_json(&mut value, secret);
                    (name.replace(secret, "[REDACTED]"), value)
                })
                .collect();
        }
        _ => {}
    }
}
