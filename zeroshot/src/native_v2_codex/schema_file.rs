use std::path::{Path, PathBuf};

use serde_json::Value;
use uuid::Uuid;

use crate::execution::process::write_new_file;

#[derive(Debug, thiserror::Error)]
pub(super) enum CodexSchemaFileError {
    #[error("provider response schema could not be serialized: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("provider response schema file could not be created: {0}")]
    Write(#[source] std::io::Error),
}

pub(super) struct CodexSchemaFile {
    path: PathBuf,
}

impl CodexSchemaFile {
    pub(super) fn create(
        runtime_home: &Path,
        schema: &Value,
    ) -> Result<Self, CodexSchemaFileError> {
        let path = runtime_home.join(format!("response-schema-{}.json", Uuid::now_v7()));
        let bytes = serde_json::to_vec(schema).map_err(CodexSchemaFileError::Serialize)?;
        // Hosted children have a distinct uid. The containing runtime home is mode 0700,
        // so making this non-secret contract world-readable only exposes it to that child.
        write_new_file(&path, &bytes, 0o444).map_err(CodexSchemaFileError::Write)?;
        Ok(Self { path })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CodexSchemaFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
#[path = "schema_file/tests.rs"]
mod tests;
