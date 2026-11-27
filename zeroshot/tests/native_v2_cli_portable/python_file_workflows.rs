//! The public Python file example through the shipped CLI, on each native CI lane.
use super::{Fixture, success};
use std::path::{Path, PathBuf};

#[test]
fn sdk_file_workflow_preserves_publications_and_live_local_paths() {
    let fixture = Fixture::new();
    fixture.write(
        "legacy-discovery.json",
        &serde_json::to_value(
            openengine_cluster_protocol::TargetDiscoveryDocument::direct(
                openengine_cluster_protocol::TargetAuthentication::None,
            ),
        )
        .unwrap(),
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let python = root.join(if cfg!(windows) {
        ".venv/Scripts/python.exe"
    } else {
        ".venv/bin/python"
    });
    let python = if python.is_file() {
        python
    } else {
        PathBuf::from(if cfg!(windows) { "python" } else { "python3" })
    };
    let environment = fixture.command(&[]);
    let output = std::process::Command::new(python)
        .env_clear()
        .envs(
            environment
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        )
        .env("PYTHONPATH", root.join("sdks/python/src"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env(
            "ZEROSHOT_PYTHON_NATIVE_BINARY",
            env!("CARGO_BIN_EXE_zeroshot"),
        )
        .current_dir(&fixture.workspace)
        .args(["-c", include_str!("python_file_workflows.py")])
        .arg(fixture.root.path(""))
        .arg(root)
        .output()
        .unwrap();
    success(&output);
}
