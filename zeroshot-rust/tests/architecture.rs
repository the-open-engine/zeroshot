use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn product_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repository_root() -> PathBuf {
    product_root()
        .parent()
        .expect("product crate must be a root workspace member")
        .to_path_buf()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn relative_files(root: &Path, directory: &Path, output: &mut BTreeSet<String>) {
    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read directory {}: {error}", directory.display()))
    {
        let entry = entry.expect("directory entry must be readable");
        let path = entry.path();
        if path.is_dir() {
            relative_files(root, &path, output);
        } else {
            output.insert(
                path.strip_prefix(root)
                    .expect("file must be under product root")
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

fn workspace_metadata() -> Value {
    let root = repository_root();
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()
        .expect("cargo metadata must run");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("cargo metadata must emit JSON")
}

fn product_package(metadata: &Value) -> &Value {
    metadata["packages"]
        .as_array()
        .expect("metadata packages must be an array")
        .iter()
        .find(|package| package["name"] == "zeroshot-rust")
        .expect("workspace must contain zeroshot-rust")
}

fn runtime_source() -> String {
    let product = product_root();
    format!(
        "{}\n{}",
        read(&product.join("src/lib.rs")),
        read(&product.join("src/main.rs"))
    )
}

#[test]
fn product_uses_the_root_workspace_and_fixed_layout() {
    let root = repository_root();
    let product = product_root();
    assert!(root.join("Cargo.toml").is_file());
    assert!(root.join("Cargo.lock").is_file());
    assert!(!product.join("Cargo.lock").exists());
    assert!(!product.join("package.json").exists());
    assert!(!read(&product.join("Cargo.toml")).contains("[workspace]"));

    let mut files = BTreeSet::new();
    relative_files(&product, &product, &mut files);
    assert_eq!(
        files,
        BTreeSet::from([
            "Cargo.toml".to_owned(),
            "src/fault/redaction.rs".to_owned(),
            "src/fault/taxonomy.rs".to_owned(),
            "src/fault.rs".to_owned(),
            "src/lib.rs".to_owned(),
            "src/main.rs".to_owned(),
            "src/observability.rs".to_owned(),
            "tests/architecture.rs".to_owned(),
            "tests/backend_boundary.rs".to_owned(),
            "tests/fault_contract.rs".to_owned(),
            "tests/observability_contract.rs".to_owned(),
        ])
    );
}

#[test]
fn workspace_metadata_exposes_the_fixed_product_identity() {
    let metadata = workspace_metadata();
    assert_eq!(
        metadata["workspace_root"],
        repository_root().to_string_lossy().as_ref()
    );
    let targets = product_package(&metadata)["targets"]
        .as_array()
        .expect("package targets must be an array")
        .iter()
        .map(|target| {
            (
                target["name"].as_str().expect("target name").to_owned(),
                target["kind"][0].as_str().expect("target kind").to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        targets,
        BTreeSet::from([
            ("architecture".to_owned(), "test".to_owned()),
            ("backend_boundary".to_owned(), "test".to_owned()),
            ("fault_contract".to_owned(), "test".to_owned()),
            ("observability_contract".to_owned(), "test".to_owned()),
            ("zeroshot-rust".to_owned(), "bin".to_owned()),
            ("zeroshot_engine".to_owned(), "lib".to_owned()),
        ])
    );
}

#[test]
fn product_has_only_focused_rust_dependencies() {
    let metadata = workspace_metadata();
    let dependencies = product_package(&metadata)["dependencies"]
        .as_array()
        .expect("dependencies must be an array")
        .iter()
        .map(|dependency| {
            (
                dependency["name"]
                    .as_str()
                    .expect("dependency name")
                    .to_owned(),
                dependency["kind"].as_str().unwrap_or("normal").to_owned(),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        dependencies,
        BTreeSet::from([
            ("async-trait".to_owned(), "normal".to_owned()),
            (
                "openengine-cluster-protocol".to_owned(),
                "normal".to_owned(),
            ),
            ("openengine-cluster-server".to_owned(), "normal".to_owned()),
            ("serde".to_owned(), "normal".to_owned()),
            ("serde_json".to_owned(), "normal".to_owned()),
            ("tokio".to_owned(), "dev".to_owned()),
        ])
    );
}

#[test]
fn runtime_reuses_the_protocol_backend_and_production_dispatcher() {
    let runtime = runtime_source();
    for required in [
        "openengine_cluster_protocol",
        "ClusterBackend",
        "ConnectionContext",
        "InitializeResult",
        "GetResult",
        "openengine_cluster_server",
        "Dispatcher",
        "NativeBackendFactory",
    ] {
        assert!(
            runtime.contains(required),
            "missing shared seam: {required}"
        );
    }
}

#[test]
fn runtime_does_not_copy_protocol_or_server_types() {
    let runtime = runtime_source();
    for copied_type in [
        "struct JsonRpc",
        "enum JsonRpc",
        "struct Dispatcher",
        "struct ConnectionContext",
        "struct InitializeParams",
        "struct GetParams",
        "struct ClusterStatus",
        "struct ServerCapabilities",
    ] {
        assert!(
            !runtime.contains(copied_type),
            "product must not copy protocol/server type: {copied_type}"
        );
    }
}

#[test]
fn runtime_has_no_alternate_runtime_seams() {
    let runtime = runtime_source();
    for forbidden_code in [
        "std::process",
        "Command::new",
        "pub mod transport",
        "pub mod client",
        "conformance_runner",
        "trait BackendFactory",
        "struct BackendFactory",
        ".zeroshot",
    ] {
        assert!(
            !runtime.contains(forbidden_code),
            "forbidden product coupling: {forbidden_code}"
        );
    }
}

#[test]
fn runtime_has_no_future_product_concerns() {
    let words = runtime_source()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<BTreeSet<_>>();
    for forbidden_word in [
        "node",
        "npm",
        "javascript",
        "config",
        "migration",
        "fallback",
        "benchmark",
        "selector",
        "transport",
        "daemon",
        "persistence",
        "verifier",
        "scheduler",
        "worker",
        "workspace",
        "artifact",
        "provider",
    ] {
        assert!(
            !words.contains(forbidden_word),
            "forbidden future product concern: {forbidden_word}"
        );
    }
}

#[test]
fn manifest_has_no_client_testkit_or_node_dependencies() {
    let manifest = read(&product_root().join("Cargo.toml"));
    for forbidden_dependency in [
        "openengine-cluster-client",
        "openengine-cluster-testkit",
        "node",
        "npm",
    ] {
        assert!(
            !manifest.contains(forbidden_dependency),
            "forbidden product dependency: {forbidden_dependency}"
        );
    }
}
