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

fn rust_sources(relative_roots: &[&str]) -> String {
    let product = product_root();
    let mut files = BTreeSet::new();
    for relative_root in relative_roots {
        let path = product.join(relative_root);
        if path.is_dir() {
            relative_files(&product, &path, &mut files);
        } else {
            files.insert((*relative_root).to_owned());
        }
    }
    files
        .into_iter()
        .map(|path| read(&product.join(path)))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn product_uses_the_root_workspace_and_a_rust_only_layout() {
    let root = repository_root();
    let product = product_root();
    assert!(root.join("Cargo.toml").is_file());
    assert!(root.join("Cargo.lock").is_file());
    assert!(!product.join("Cargo.lock").exists());
    assert!(!product.join("package.json").exists());
    assert!(!read(&product.join("Cargo.toml")).contains("[workspace]"));

    let mut files = BTreeSet::new();
    relative_files(&product, &product, &mut files);
    for required in [
        "Cargo.toml",
        "src/fault.rs",
        "src/fault/redaction.rs",
        "src/fault/taxonomy.rs",
        "src/lib.rs",
        "src/main.rs",
        "src/observability.rs",
        "src/provider_value.rs",
        "src/issue_provider.rs",
        "src/source_code_provider.rs",
        "tests/architecture.rs",
        "tests/backend_boundary.rs",
        "tests/fault_contract.rs",
        "tests/observability_contract.rs",
        "tests/provider_contracts.rs",
        "tests/provider_bounds.rs",
    ] {
        assert!(files.contains(required), "missing product file: {required}");
    }
    for file in files {
        assert!(
            file == "Cargo.toml" || file.ends_with(".rs"),
            "native product must remain Rust-only: {file}"
        );
    }
}

#[test]
fn workspace_metadata_preserves_package_lib_and_bin_identity() {
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
            ("provider_bounds".to_owned(), "test".to_owned()),
            ("provider_contracts".to_owned(), "test".to_owned()),
            ("zeroshot-rust".to_owned(), "bin".to_owned()),
            ("zeroshot_engine".to_owned(), "lib".to_owned()),
        ])
    );
}

#[test]
fn product_dependencies_stay_inside_native_contract_and_backend_boundaries() {
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
    let allowed = BTreeSet::from([
        ("async-trait".to_owned(), "normal".to_owned()),
        (
            "openengine-cluster-protocol".to_owned(),
            "normal".to_owned(),
        ),
        ("openengine-cluster-server".to_owned(), "normal".to_owned()),
        ("serde".to_owned(), "normal".to_owned()),
        ("serde_json".to_owned(), "normal".to_owned()),
        ("thiserror".to_owned(), "normal".to_owned()),
        ("tokio".to_owned(), "dev".to_owned()),
    ]);
    assert_eq!(dependencies, allowed);
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
    ] {
        assert!(
            !words.contains(forbidden_word),
            "forbidden future product concern: {forbidden_word}"
        );
    }
}

#[test]
fn provider_contracts_add_no_ledger_workspace_worker_protocol_adapter_or_fault_behavior() {
    let product = product_root();
    let provider_value = rust_sources(&["src/provider_value.rs", "src/provider_value"]);
    let contracts = rust_sources(&[
        "src/issue_provider.rs",
        "src/issue_provider",
        "src/source_code_provider.rs",
        "src/source_code_provider",
    ]);
    assert!(
        read(&product.join("src/lib.rs")).contains("mod provider_value;"),
        "bounded provider helpers must remain product-private"
    );
    for forbidden in [
        "pub trait Provider",
        "PlatformProfile",
        "ChangeProvider",
        "CommonProviderId",
    ] {
        assert!(
            !provider_value.contains(forbidden),
            "provider_value must not expose a common provider abstraction: {forbidden}"
        );
    }
    for forbidden in [
        "ClusterLedger",
        "rusqlite",
        "WorkspaceLease",
        "WorkerRegistry",
        "WorkerProvider",
        "EngineFault",
        "openengine_cluster_protocol",
        "openengine_cluster_server",
        "Adapter",
    ] {
        assert!(
            !contracts.contains(forbidden),
            "provider contracts crossed an owned boundary: {forbidden}"
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
