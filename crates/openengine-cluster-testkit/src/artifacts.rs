use crate::EmptyBackend;
use anyhow::{bail, Context, Result};
use openengine_cluster_protocol::{
    GetParams, GetResult, InitializeParams, InitializeResult, JsonRpcErrorResponse, JsonRpcRequest,
    JsonRpcSuccess, PROTOCOL_VERSION,
};
use openengine_cluster_server::{ConnectionContext, Dispatcher};
use schemars::{schema_for, JsonSchema};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const ARTIFACT_DIRECTORY: &str = "protocol/openengine-cluster/v1";

#[derive(JsonSchema)]
#[doc(hidden)]
pub struct ImplementedProtocolSchema {
    pub initialize_request: JsonRpcRequest<InitializeParams>,
    pub initialize_params: InitializeParams,
    pub initialize_response: JsonRpcSuccess<InitializeResult>,
    pub get_request: JsonRpcRequest<GetParams>,
    pub get_params: GetParams,
    pub get_response: JsonRpcSuccess<GetResult>,
    pub error_response: JsonRpcErrorResponse,
}

pub async fn generated_artifacts() -> Result<BTreeMap<PathBuf, String>> {
    let mut artifacts = BTreeMap::new();
    let schema = schema_for!(ImplementedProtocolSchema);
    artifacts.insert(
        PathBuf::from("schema.json"),
        pretty_json(&schema).context("serialize protocol JSON Schema")?,
    );
    artifacts.insert(
        PathBuf::from("openrpc.json"),
        pretty_json(&openrpc_document()).context("serialize OpenRPC document")?,
    );

    let dispatcher = Dispatcher::new(EmptyBackend, ConnectionContext::new("artifact-generator"));
    let golden_requests = [
        (
            "goldens/initialize.ndjson",
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":PROTOCOL_VERSION}}).to_string(),
        ),
        (
            "goldens/get-empty.ndjson",
            json!({"jsonrpc":"2.0","id":"get-1","method":"get","params":{}}).to_string(),
        ),
        (
            "goldens/incompatible-version.ndjson",
            json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"openengine.cluster/v0"}}).to_string(),
        ),
        (
            "goldens/invalid-params.ndjson",
            r#"{"jsonrpc":"2.0","id":3,"method":"get","params":[]}"#.to_owned(),
        ),
        (
            "goldens/unknown-method.ndjson",
            r#"{"jsonrpc":"2.0","id":4,"method":"unknown","params":{}}"#.to_owned(),
        ),
        (
            "goldens/malformed-request.ndjson",
            r#"{"jsonrpc":"2.0","id":5,"params":{}}"#.to_owned(),
        ),
        (
            "goldens/rejected-batch.ndjson",
            r#"[{"jsonrpc":"2.0","id":6,"method":"get","params":{}}]"#.to_owned(),
        ),
    ];
    for (path, request) in golden_requests {
        let response = dispatcher.dispatch_line(&request).await;
        artifacts.insert(PathBuf::from(path), format!("{request}\n{response}\n"));
    }
    Ok(artifacts)
}

pub async fn write_artifacts(repository_root: &Path) -> Result<()> {
    let root = repository_root.join(ARTIFACT_DIRECTORY);
    if root.exists() {
        fs::remove_dir_all(&root)
            .with_context(|| format!("remove stale generated artifacts at {}", root.display()))?;
    }
    for (relative_path, contents) in generated_artifacts().await? {
        let path = root.join(relative_path);
        fs::create_dir_all(path.parent().expect("artifact path has parent"))
            .with_context(|| format!("create artifact directory for {}", path.display()))?;
        fs::write(&path, contents)
            .with_context(|| format!("write generated artifact {}", path.display()))?;
    }
    Ok(())
}

pub async fn check_artifacts(repository_root: &Path) -> Result<()> {
    let root = repository_root.join(ARTIFACT_DIRECTORY);
    let expected = generated_artifacts().await?;
    let expected_paths: BTreeSet<PathBuf> = expected.keys().cloned().collect();
    let actual_paths = collect_relative_files(&root)?;
    if actual_paths != expected_paths {
        let missing: Vec<_> = expected_paths.difference(&actual_paths).collect();
        let stale: Vec<_> = actual_paths.difference(&expected_paths).collect();
        bail!("generated artifact set drifted; missing={missing:?}; stale={stale:?}");
    }
    for (relative_path, expected_contents) in expected {
        let path = root.join(&relative_path);
        let actual = fs::read_to_string(&path)
            .with_context(|| format!("read generated artifact {}", path.display()))?;
        if actual != expected_contents {
            let mismatch = actual
                .bytes()
                .zip(expected_contents.bytes())
                .position(|(actual, expected)| actual != expected)
                .unwrap_or_else(|| actual.len().min(expected_contents.len()));
            bail!(
                "generated artifact drifted: {} (first mismatch byte {mismatch}; actual {} bytes; expected {} bytes). Run the generator with --write",
                path.display(),
                actual.len(),
                expected_contents.len()
            );
        }
    }
    Ok(())
}

fn collect_relative_files(root: &Path) -> Result<BTreeSet<PathBuf>> {
    if !root.is_dir() {
        bail!(
            "generated artifact directory is missing: {}. Run the generator with --write",
            root.display()
        );
    }
    let mut files = BTreeSet::new();
    collect_files_recursive(root, root, &mut files)?;
    Ok(files)
}

fn collect_files_recursive(
    root: &Path,
    directory: &Path,
    files: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(directory)
        .with_context(|| format!("read artifact directory {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files_recursive(root, &path, files)?;
        } else {
            files.insert(path.strip_prefix(root)?.to_owned());
        }
    }
    Ok(())
}

fn pretty_json(value: &impl serde::Serialize) -> serde_json::Result<String> {
    serde_json::to_string_pretty(value).map(|serialized| format!("{serialized}\n"))
}

fn openrpc_document() -> serde_json::Value {
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
                    "schema": {"const": PROTOCOL_VERSION, "type": "string"}
                }],
                "result": {
                    "name": "result",
                    "schema": {"$ref": "schema.json#/$defs/InitializeResult"}
                }
            },
            {
                "name": "get",
                "paramStructure": "by-name",
                "params": [],
                "result": {
                    "name": "result",
                    "schema": {"$ref": "schema.json#/$defs/GetResult"}
                }
            }
        ]
    })
}
