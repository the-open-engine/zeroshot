use super::{Fixture, success};
use openengine_cluster_protocol::{TargetAuthentication, TargetDiscoveryDocument};
use std::path::{Path, PathBuf};

#[test]
fn python_checkpoint_opt_out_uses_real_cli_discovery_and_preserves_false() {
    let fixture = Fixture::new();
    fixture.write(
        "discovery.json",
        &serde_json::to_value(
            TargetDiscoveryDocument::direct(TargetAuthentication::None).with_git_transport(),
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
        .args(["-c", include_str!("checkpoint_opt_out.py")])
        .arg(fixture.root.path(""))
        .output()
        .unwrap();
    success(&output);
}
