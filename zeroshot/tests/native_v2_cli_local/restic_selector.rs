//! `ZEROSHOT_RESTIC` selects the checkpoint tool for the detached local controller, and a
//! controller that cannot start reports why to the submitting CLI.

use super::*;

const STARTUP_EXITED: &str = "controller exited before becoming ready";
const SELECTOR_UNUSABLE: &str = "ZEROSHOT_RESTIC does not name a restic executable";

#[tokio::test(flavor = "multi_thread")]
async fn detached_run_reports_why_its_controller_could_not_start() {
    let mut fixture = LocalFixture::new();
    let missing = fixture.root.path("missing-restic");
    fixture
        .harness_environment
        .insert("ZEROSHOT_RESTIC".to_owned(), missing.display().to_string());

    let output = fixture
        .run("Unusable Restic selector", "finish", true)
        .await;

    assert!(!output.status.success(), "run must fail: {output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(STARTUP_EXITED) && stderr.contains(SELECTOR_UNUSABLE),
        "the startup failure must name its cause:\n{stderr}"
    );
    assert!(!stderr.contains(DECLARED_KEY), "{stderr}");
}

#[tokio::test(flavor = "multi_thread")]
async fn detached_controller_uses_the_selected_restic() {
    let mut fixture = LocalFixture::new();
    let directory = fixture.root.path("selected-restic");
    fs::create_dir_all(&directory).assert_value_with("create selected Restic directory");
    let marker = fixture.root.path("selected-restic-ran");
    let marker_path = marker.to_str().assert_value_with("marker path");
    assert!(
        !marker_path.contains('\''),
        "marker path must not need quoting"
    );
    let restic = directory.join("restic");
    openengine_cluster_testkit::fixture::write_executable(
        &restic,
        format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{marker_path}'\nexit 1\n"),
        0o755,
    )
    .assert_value_with("write selected Restic");
    // A relative selector resolves against the submitting CLI's working directory.
    assert_eq!(fixture.working_directory, fixture.root.path("repository"));
    fixture.harness_environment.insert(
        "ZEROSHOT_RESTIC".to_owned(),
        "../selected-restic/restic".to_owned(),
    );

    let run_id = fixture.submit_detached("finish").await;

    let deadline = Instant::now() + CLI_TIMEOUT;
    while !marker.exists() {
        assert!(
            Instant::now() < deadline,
            "the controller did not run the selected Restic"
        );
        sleep(Duration::from_millis(30)).await;
    }
    wait_for_exit(fixture.ready_pid(&run_id)).await;
}
