#![cfg(unix)]

use std::process::Command;

use openengine_cluster_testkit::fixture::{TemporaryDirectory, python3_script, write_executable};

#[test]
fn python_fixture_preserves_arguments_with_a_cleared_environment() {
    let root = TemporaryDirectory::for_test("python-fixture's path");
    let script = root.path("fixture");
    write_executable(
        &script,
        python3_script("#!/usr/bin/env python3\nimport sys; print(sys.argv[1])\n"),
        0o700,
    )
    .expect("write Python fixture");
    let output = Command::new(script)
        .arg("a quoted ' argument")
        .env_clear()
        .output()
        .expect("run Python fixture without an inherited environment");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"a quoted ' argument\n");
}
