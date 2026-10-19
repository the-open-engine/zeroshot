//! `ZEROSHOT_RESTIC` selects the checkpoint tool for the detached local controller, and a
//! controller that cannot start reports why to the submitting CLI.

use super::*;

const STARTUP_EXITED: &str = "controller exited before becoming ready";
const SELECTOR_UNUSABLE: &str = "ZEROSHOT_RESTIC does not name a restic executable";

fn test_restic() -> PathBuf {
    [
        Path::new(env!("CARGO_BIN_EXE_zeroshot")).with_file_name("restic"),
        PathBuf::from("/usr/bin/restic"),
        PathBuf::from("/bin/restic"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
    .assert_value_with("local CLI tests need bundled or system-installed Restic")
}

#[tokio::test(flavor = "multi_thread")]
async fn relative_restic_selector_checkpoints_a_successful_local_run() {
    let mut fixture = LocalFixture::new();
    let directory = fixture.root.path("selected-restic");
    fs::create_dir_all(&directory).assert_value_with("create selected Restic directory");
    let invocations = fixture.root.path("selected-restic.log");
    openengine_cluster_testkit::fixture::write_executable(
        &directory.join("restic"),
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> {}\nexec {} \"$@\"\n",
            shell_literal(invocations.to_str().assert_value_with("log path")),
            shell_literal(test_restic().to_str().assert_value_with("restic path")),
        ),
        0o755,
    )
    .assert_value_with("write selected Restic");
    // A relative selector resolves against the submitting CLI's working directory.
    assert_eq!(fixture.working_directory, fixture.root.path("repository"));
    fixture.harness_environment.insert(
        "ZEROSHOT_RESTIC".to_owned(),
        "../selected-restic/restic".to_owned(),
    );

    let output = fixture.run("Selected Restic", "finish", false).await;

    assert_success(&output, "local run with a relative ZEROSHOT_RESTIC");
    let invoked = fs::read_to_string(&invocations).unwrap_or_default();
    assert!(
        !invoked.trim().is_empty(),
        "the controller did not run the selected Restic"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unusable_restic_selector_names_the_controller_startup_failure() {
    let mut fixture = LocalFixture::new();
    let directory = fixture.root.path("restic-directory");
    fs::create_dir_all(&directory).assert_value_with("create Restic directory");
    for (selector, cause) in [
        (
            fixture.root.path("missing-restic"),
            "No such file or directory",
        ),
        (directory, "restic executable is not a file"),
    ] {
        fixture
            .harness_environment
            .insert("ZEROSHOT_RESTIC".to_owned(), selector.display().to_string());

        let output = fixture
            .run("Unusable Restic selector", "finish", true)
            .await;

        assert!(!output.status.success(), "run must fail: {output:?}");
        let stderr = String::from_utf8(output.stderr).assert_value_with("UTF-8 stderr");
        let expected = format!(
            "local controller operation failed: {STARTUP_EXITED}: portable controller I/O \
             failed: {SELECTOR_UNUSABLE}: {cause}"
        );
        assert!(
            stderr.contains(&expected),
            "the startup failure must name its cause {expected:?}:\n{stderr}"
        );
        assert!(!stderr.contains(DECLARED_KEY), "{stderr}");
        assert_eq!(stderr.lines().count(), 1, "one printable line:\n{stderr}");
        assert!(
            !stderr.trim_end().chars().any(char::is_control),
            "{stderr:?}"
        );
    }
    for run in fs::read_dir(fixture.state.join("runs")).assert_value_with("list runs") {
        let storage = run.assert_value_with("run entry").path();
        assert!(!storage.join("controller.startup-failure.txt").exists());
        assert!(!storage.join("controller.bootstrap.json").exists());
    }
}
