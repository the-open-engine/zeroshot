use std::error::Error as _;

use thiserror::Error;
use zeroshot_engine::native_v2_cli::NativeV2CliError;

use super::TargetRecord;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct TargetAuthorityError {
    message: String,
    disconnected: bool,
    remote: Option<TargetRemoteError>,
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
            disconnected: false,
            remote: None,
        }
    }

    pub(super) fn disconnected(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            disconnected: true,
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
        Self::disconnected(format!("{operation} failed: {reason}"))
    }

    pub(super) fn remote(
        code: impl Into<String>,
        message: impl Into<String>,
        details: Option<serde_json::Value>,
    ) -> Self {
        Self {
            message: message.into(),
            disconnected: false,
            remote: Some(TargetRemoteError {
                code: code.into(),
                details,
            }),
        }
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
        if self.disconnected {
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
}
