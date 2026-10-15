//! Caller-owned renewable GitHub authority for a trusted local delivery controller.

use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::execution::platform::{FileAccess, private_file};
use crate::native_v2_runner::EnvironmentRefreshError;

pub(crate) const GITHUB_TOKEN_FILE_ENV: &str = "ZEROSHOT_GITHUB_TOKEN_FILE";

/// A path outside the checkout, reopened for each issuance. Never passed to provider children.
#[derive(Clone)]
pub struct TrustedGitHubTokenFile {
    path: PathBuf,
}

impl TrustedGitHubTokenFile {
    /// Resolve aliases before checking that rotating credentials cannot enter the checkout.
    pub fn new(path: PathBuf, workspace: &Path) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(invalid_path());
        }
        let parent = path.parent().ok_or_else(invalid_path)?.canonicalize()?;
        let path = parent.join(path.file_name().ok_or_else(invalid_path)?);
        let workspace = workspace.canonicalize()?;
        if path.starts_with(workspace) {
            return Err(invalid_path());
        }
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn read_rotated(&self, rejected: &str) -> Result<String, EnvironmentRefreshError> {
        let token = self.read()?;
        if token == rejected {
            Err(EnvironmentRefreshError::Unavailable)
        } else {
            Ok(token)
        }
    }

    pub(crate) fn read(&self) -> Result<String, EnvironmentRefreshError> {
        let file =
            private_file(&self.path, FileAccess::Read).map_err(|error| match error.kind() {
                io::ErrorKind::PermissionDenied => EnvironmentRefreshError::Refused,
                io::ErrorKind::NotFound => EnvironmentRefreshError::Unavailable,
                _ => EnvironmentRefreshError::InvalidResponse,
            })?;
        let mut bytes = Vec::new();
        file.take(super::MAX_TOKEN_BYTES as u64 + 3)
            .read_to_end(&mut bytes)
            .map_err(|_| EnvironmentRefreshError::Unavailable)?;
        let value =
            std::str::from_utf8(&bytes).map_err(|_| EnvironmentRefreshError::InvalidResponse)?;
        let token = value
            .strip_suffix("\r\n")
            .or_else(|| value.strip_suffix('\n'))
            .unwrap_or(value);
        validate_token(token).map_err(|_| EnvironmentRefreshError::InvalidResponse)?;
        Ok(token.to_owned())
    }
}

pub(crate) fn validate_token(value: &str) -> Result<(), EnvironmentRefreshError> {
    if value.is_empty()
        || value.len() > super::MAX_TOKEN_BYTES
        || value.bytes().any(|byte| !byte.is_ascii_graphic())
    {
        Err(EnvironmentRefreshError::Refused)
    } else {
        Ok(())
    }
}

fn invalid_path() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "trusted GitHub credential file must be outside the workspace",
    )
}

impl fmt::Debug for TrustedGitHubTokenFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TrustedGitHubTokenFile([PRIVATE])")
    }
}

#[cfg(test)]
#[path = "token_file/tests.rs"]
mod tests;
