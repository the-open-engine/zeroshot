//! Generated file-service contract, deliberately separate from OECP and worker artifact schemas.

use openengine_cluster_protocol::{FileEntry, FileRequest, FileResponse, RunFileInput};
use schemars::{JsonSchema, schema_for};

use crate::artifacts::{Artifact, json_artifact};
use crate::assertions::AssertValue;

#[derive(JsonSchema)]
pub struct TargetFilesSchema {
    pub request: FileRequest,
    pub response: FileResponse,
    pub entry: FileEntry,
    pub input: RunFileInput,
}

pub(crate) fn artifacts() -> Vec<Artifact> {
    vec![json_artifact(
        "protocol/openengine-cluster/v1/target-files.schema.json".to_owned(),
        serde_json::to_value(schema_for!(TargetFilesSchema)).assert_value(),
    )]
}
