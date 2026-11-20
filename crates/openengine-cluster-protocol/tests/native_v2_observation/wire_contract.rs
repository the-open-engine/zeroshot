//! The observation suite already runs in the Linux, Windows and macOS CI lanes.
//! Keep frozen release dependencies outside product package selection while exercising their
//! real initialization controls and the current runtime/HTTP producer boundaries here.

use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn current_and_released_wire_contracts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("protocol crate belongs to the repository workspace");
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
        .join("wire-contract");
    let commands: &[(&str, &[&str])] = &[
        (
            "current",
            &["--package", "zeroshot", "--lib", "--", "wire_contract::"],
        ),
        (
            "current",
            &[
                "--package",
                "zeroshot",
                "--bin",
                "zeroshot",
                "--",
                "wire_contract::",
            ],
        ),
        (
            "released",
            &[
                "--manifest-path",
                "crates/openengine-cluster-testkit/tests/released-wire-contract/Cargo.toml",
            ],
        ),
    ];
    for (workspace, args) in commands {
        // These leaf suites never select this parent suite. A separate target directory preserves
        // the surrounding job's CLI feature profile; one worker bounds child compilation.
        let mut command = Command::new(env!("CARGO"));
        // Cargo exports the parent's package metadata to tests. Build scripts watch these
        // variables, so inheriting them would invalidate the Windows prebuilds.
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(|name| {
                name.starts_with("CARGO_MANIFEST_") || name.starts_with("CARGO_PKG_")
            }) {
                command.env_remove(name);
            }
        }
        let status = command
            .args(["test", "--locked", "--jobs", "1"])
            .args(*args)
            .current_dir(root)
            .env("CARGO_TARGET_DIR", target.join(workspace))
            .status()
            .expect("Cargo must run the wire-contract controls");
        assert!(status.success(), "cargo test {args:?} failed: {status}");
    }
}
