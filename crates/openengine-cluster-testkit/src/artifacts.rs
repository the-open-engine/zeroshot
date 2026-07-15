use std::fs;
use std::path::{Path, PathBuf};

use openengine_cluster_protocol::{
    GetParams, GetResult, InitializeParams, InitializeResult, JsonRpcRequest, JsonRpcResponse,
};
use openengine_cluster_server::{ConnectionContext, Dispatcher};
use schemars::{schema_for, JsonSchema};
use serde_json::{json, Value};
use thiserror::Error;

use crate::EmptyBackend;

const ROOT: &str = "protocol/openengine-cluster/v1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Artifact {
    pub relative_path: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("generated artifact is missing: {0}")]
    Missing(PathBuf),
    #[error("generated artifact has byte drift: {0}")]
    Drift(PathBuf),
    #[error("generated artifact I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

#[derive(JsonSchema)]
pub struct ImplementedProtocolSchema {
    pub initialize_request: JsonRpcRequest<InitializeParams>,
    pub initialize_response: JsonRpcResponse<InitializeResult>,
    pub get_request: JsonRpcRequest<GetParams>,
    pub get_response: JsonRpcResponse<GetResult>,
}

pub async fn generate_artifacts() -> Vec<Artifact> {
    let schema = serde_json::to_value(schema_for!(ImplementedProtocolSchema))
        .expect("JSON Schema serialization must succeed");
    let openrpc = openrpc_document();
    let dispatcher = Dispatcher::new(EmptyBackend, ConnectionContext::default());

    let cases = [
        (
            "initialize.ndjson",
            r#"{"jsonrpc":"2.0","id":"init-1","method":"initialize","params":{"protocolVersion":"openengine.cluster/v1"}}"#,
        ),
        (
            "get-empty.ndjson",
            r#"{"jsonrpc":"2.0","id":2,"method":"get","params":{}}"#,
        ),
        (
            "incompatible-version.ndjson",
            r#"{"jsonrpc":"2.0","id":3,"method":"initialize","params":{"protocolVersion":"openengine.cluster/v0"}}"#,
        ),
        (
            "invalid-params.ndjson",
            r#"{"jsonrpc":"2.0","id":4,"method":"get","params":[]}"#,
        ),
        (
            "unknown-method.ndjson",
            r#"{"jsonrpc":"2.0","id":5,"method":"cluster.missing","params":{}}"#,
        ),
        (
            "malformed-request.ndjson",
            r#"{"jsonrpc":"1.0","id":6,"method":"get","params":{}}"#,
        ),
        ("rejected-batch.ndjson", r#"[]"#),
    ];

    let mut artifacts = vec![
        json_artifact(format!("{ROOT}/schema.json"), schema),
        json_artifact(format!("{ROOT}/openrpc.json"), openrpc),
    ];
    for (name, request) in cases {
        let response = dispatcher.dispatch(request).await;
        artifacts.push(Artifact {
            relative_path: format!("{ROOT}/goldens/{name}"),
            bytes: format!("{request}\n{response}\n").into_bytes(),
        });
    }
    artifacts
}

pub async fn check_artifacts(workspace: &Path) -> Result<(), ArtifactError> {
    for artifact in generate_artifacts().await {
        let path = workspace.join(&artifact.relative_path);
        let actual = match fs::read(&path) {
            Ok(actual) => actual,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(ArtifactError::Missing(path));
            }
            Err(source) => return Err(ArtifactError::Io { path, source }),
        };
        if actual != artifact.bytes {
            return Err(ArtifactError::Drift(path));
        }
    }
    Ok(())
}

pub async fn write_artifacts(workspace: &Path) -> Result<(), ArtifactError> {
    for artifact in generate_artifacts().await {
        let path = workspace.join(&artifact.relative_path);
        let parent = path
            .parent()
            .expect("every generated artifact must have a parent directory");
        fs::create_dir_all(parent).map_err(|source| ArtifactError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        fs::write(&path, artifact.bytes).map_err(|source| ArtifactError::Io { path, source })?;
    }
    Ok(())
}

#[must_use]
pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("testkit crate must be two directories below the workspace")
        .to_path_buf()
}

fn json_artifact(relative_path: String, value: Value) -> Artifact {
    let mut bytes = serde_json::to_vec_pretty(&value).expect("artifact serialization must succeed");
    bytes.push(b'\n');
    Artifact {
        relative_path,
        bytes,
    }
}

fn openrpc_document() -> Value {
    json!({
        "openrpc": "1.3.2",
        "info": {
            "title": "Open Engine Cluster Protocol",
            "version": "1.0.0"
        },
        "methods": [
            {
                "name": "initialize",
                "paramStructure": "by-name",
                "params": [{
                    "name": "protocolVersion",
                    "required": true,
                    "schema": {
                        "type": "string",
                        "const": "openengine.cluster/v1"
                    }
                }],
                "result": {
                    "name": "initializeResult",
                    "schema": { "$ref": "schema.json#/$defs/InitializeResult" }
                }
            },
            {
                "name": "get",
                "paramStructure": "by-name",
                "params": [{
                    "name": "atCursor",
                    "required": false,
                    "schema": {
                        "type": ["string", "null"]
                    }
                }],
                "result": {
                    "name": "getResult",
                    "schema": { "$ref": "schema.json#/$defs/GetResult" }
                }
            }
        ]
    })
}
