use std::os::unix::fs::PermissionsExt as _;

use openengine_cluster_testkit::assertions::AssertAt as _;

use super::*;

const SHELL_SECRET: &str = "synthetic-private-shell-value";
const CAUSE: &str = "recognizable Restic cause";

fn fixture() -> LocalFixture {
    let mut fixture = LocalFixture::new();
    // Environment-cleared instrumented children must write profiles outside the checkpoint.
    fixture.working_directory = fixture.root.path("");
    fixture.cwd = Some(fixture.repository.clone());
    let selector = fixture.root.path("restic-wrapper");
    let failure = fixture.root.path("fail-operation");
    fs::write(&failure, "").assert_value();
    openengine_cluster_testkit::fixture::write_executable(
        &selector,
        format!(
            "#!/bin/sh\noperation=\nfor argument in \"$@\"; do\n\
             case \"$argument\" in\nbackup) operation=backup ;;\n\
             restore) operation=workspace ;;\n--include) operation=conversations ;;\nesac\ndone\n\
             if test -n \"$operation\" && test \"$(cat {failure})\" = \"$operation\"; then\n\
             printf '{CAUSE}: %s %s %s\\n' {key} {shell} \"$(cat \"$RESTIC_PASSWORD_FILE\")\" >&2\n\
             exit 3\nfi\nexec {restic} \"$@\"\n",
            failure = shell_literal(failure.to_str().assert_value()),
            key = shell_literal(DECLARED_KEY),
            shell = shell_literal(SHELL_SECRET),
            restic = shell_literal(restic_selector::test_restic().to_str().assert_value()),
        ),
        0o700,
    )
    .assert_value();
    fixture.harness_environment.extend([
        ("ZEROSHOT_RESTIC".to_owned(), selector.display().to_string()),
        (
            "UNRECOGNIZED_PRIVATE_VALUE".to_owned(),
            SHELL_SECRET.to_owned(),
        ),
    ]);
    fixture
}

fn assert_reports(fixture: &LocalFixture, count: usize) {
    let repositories = fs::read_dir(fixture.state.join("checkpoint-repositories"))
        .assert_value()
        .map(|entry| entry.assert_value().path())
        .collect::<Vec<_>>();
    assert_eq!(repositories.len(), 1);
    let repository = &repositories[0];
    let password = fs::read_to_string(repository.join("password")).assert_value();
    let diagnostics = repository.join("diagnostics");
    let reports = fs::read_dir(&diagnostics)
        .assert_value()
        .map(|entry| entry.assert_value().path())
        .collect::<Vec<_>>();
    assert_eq!(
        reports
            .iter()
            .filter(|path| fs::read_to_string(path).assert_value().contains(CAUSE))
            .count(),
        count
    );
    assert_eq!(
        fs::metadata(diagnostics)
            .assert_value()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    for path in reports {
        let report = fs::read_to_string(&path).assert_value();
        assert!(report.len() <= 4096);
        assert!(
            !report.contains(DECLARED_KEY),
            "stored connection value leaked"
        );
        assert!(
            !report.contains(SHELL_SECRET),
            "private invoking-shell value leaked"
        );
        assert!(
            !report.contains(password.trim()),
            "repository password leaked"
        );
        assert_eq!(
            fs::metadata(path).assert_value().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restic_capture_redacts_credentials_from_the_private_controller_bootstrap() {
    let fixture = fixture();
    fixture.connection_set().await;
    fs::write(fixture.root.path("fail-operation"), "backup").assert_value();
    // This CLI has no inline API key: the connection value comes only from private storage.
    let failed = timeout(
        SUSPENSION_TIMEOUT,
        fixture
            .command_with_inline(
                &["run", "--title", "Private checkpoint diagnostics"],
                "finish",
                false,
            )
            .arg("--cwd")
            .arg(&fixture.repository)
            .arg("--graph")
            .arg(&fixture.graph)
            .arg("--input")
            .arg(&fixture.input)
            .arg("--runtime-config")
            .arg(&fixture.runtime)
            .output(),
    )
    .await
    .assert_value()
    .assert_value();
    assert!(!failed.status.success());
    assert!(!failed.stdout.is_empty(), "{failed:?}");
    let receipt = json_lines(&failed.stdout);
    let run = receipt.as_slice().assert_at(0)["runId"]
        .as_str()
        .assert_value();
    fixture.wait_terminal(run, "finish", "runtime_failed").await;
    wait_for_exit(fixture.ready_pid(run)).await;
    assert_reports(&fixture, 1);
    let logs = fixture.output(&["logs", run], "finish").await;
    assert_success(&logs, "checkpoint failure log replay");
    let public = format!("{failed:?}\n{}", String::from_utf8_lossy(&logs.stdout));
    assert!(
        public.contains("restic backup failed (exit status: 3); private diagnostic: diagnostics/")
    );
    for secret in [DECLARED_KEY, SHELL_SECRET, CAUSE] {
        assert!(!public.contains(secret));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restic_restores_redact_the_successor_bootstrap_credentials() {
    let fixture = fixture();
    let failed = fixture
        .run("Checkpoint restore diagnostics", "fail", false)
        .await;
    assert!(!failed.status.success());
    let receipt = json_lines(&failed.stdout);
    let run = receipt.as_slice().assert_at(0)["runId"]
        .as_str()
        .assert_value();
    fixture.wait_terminal(run, "fail", "worker_failed").await;
    wait_for_exit(fixture.ready_pid(run)).await;
    let catalog = fixture.json(&["checkpoints", run], "finish").await;
    let checkpoint = catalog["checkpoints"][0]["checkpointId"]
        .as_str()
        .assert_value();
    let resume = [
        "resume",
        run,
        "--from-checkpoint",
        checkpoint,
        "--discard-workspace-changes",
    ];

    fs::write(fixture.root.path("fail-operation"), "workspace").assert_value();
    let failed_restore = fixture.output(&resume, "finish").await;
    assert!(!failed_restore.status.success());
    let error = String::from_utf8_lossy(&failed_restore.stderr);
    assert!(
        error.contains("restic restore failed (exit status: 3); private diagnostic:"),
        "{error}"
    );
    assert_reports(&fixture, 1);

    // Workspace restore now succeeds; the independent conversation extraction fails instead.
    fs::write(fixture.root.path("fail-operation"), "conversations").assert_value();
    let resumed = fixture.json(&resume, "fail").await;
    let successor = resumed["runId"].as_str().assert_value();
    fixture
        .wait_terminal(successor, "fail", "worker_failed")
        .await;
    wait_for_exit(fixture.ready_pid(successor)).await;
    assert_reports(&fixture, 2);
}
