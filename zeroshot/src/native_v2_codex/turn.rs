use std::path::Path;

use crate::native_v2_capsule::provider_process::{ProviderExecutionFiles, ProviderProcess};

use super::schema_file::CodexSchemaFile;

pub(super) struct CodexCommandInput<'a> {
    pub(super) resume: Option<&'a str>,
    pub(super) files: &'a ProviderExecutionFiles,
    pub(super) schema_path: &'a Path,
}

pub(super) struct CodexTurnProcess {
    pub(super) process: ProviderProcess,
    pub(super) _schema: CodexSchemaFile,
}

pub(super) enum CodexTurnProcessOpen {
    Ready(CodexTurnProcess),
    ProviderFailure(String),
}
