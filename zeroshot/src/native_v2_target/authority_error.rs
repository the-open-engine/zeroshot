use thiserror::Error;
use zeroshot_engine::native_v2_cli::NativeV2CliError;

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

    pub(super) fn into_cli(self) -> NativeV2CliError {
        if self.disconnected {
            NativeV2CliError::Disconnected
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
