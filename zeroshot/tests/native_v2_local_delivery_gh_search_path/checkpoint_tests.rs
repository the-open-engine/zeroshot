//! Restore checkpoint-only delivery programs before the one forge admission request.

use super::*;
use openengine_cluster_testkit::assertions::JsonAt;

const PROJECT_READ: &str = "checkpoint-project-read";
const PROXY: &str = "http://caller:checkpoint-proxy-secret@127.0.0.1:9";

#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_resume_checks_the_restored_gh_once_before_agents_start() {
    let fixture = DeliveryFixture::new();
    let tools = fixture.repository.join("tools");
    fs::create_dir(&tools).assert_value();
    write_checkpoint_gh(&fixture, &tools.join("gh"));
    git(&fixture.repository, &["add", "tools/gh"]);
    git(
        &fixture.repository,
        &["commit", "-m", "carry delivery executable"],
    );
    // Exclude system gh while retaining the host programs the fake harness and checkpoints need.
    let host = fixture.root.path("host-tools");
    fs::create_dir(&host).assert_value();
    for program in ["cat", "grep"] {
        std::os::unix::fs::symlink(host_executable(program), host.join(program)).assert_value();
    }
    let sibling = Path::new(env!("CARGO_BIN_EXE_zeroshot")).with_file_name("restic");
    let restic = if sibling.is_file() {
        sibling
    } else {
        host_executable("restic")
    };
    std::os::unix::fs::symlink(restic, host.join("restic")).assert_value();
    let path = search_path(&[PathBuf::from("tools"), fixture.bin.clone(), host]);
    let run = fixture.run("--pr", &path).await;
    assert!(
        !run.status.success() && fixture.codex_log.exists(),
        "precondition: the fake delivery leaves a failed run with worker checkpoints: {run:?}"
    );
    let run_id = receipt_id(&run);
    let checkpoints = json_output(&fixture, &["checkpoints", &run_id], &path).await;
    let checkpoint = checkpoints
        .assert_key("checkpoints")
        .as_array()
        .assert_value()
        .iter()
        .find(|entry| entry.assert_key("node") == "worker")
        .assert_value()
        .assert_key("checkpointId")
        .as_str()
        .assert_value();
    assert_eq!(project_reads(&fixture), 1);
    let original_turns = fs::read(&fixture.codex_log).assert_value();
    fs::remove_file(tools.join("gh")).assert_value();
    fs::write(fixture.root.path("check-resume-network"), b"check").assert_value();

    let plain = output(&fixture, &["resume", &run_id], &path).await;
    assert!(
        !plain.status.success()
            && String::from_utf8_lossy(&plain.stderr).contains("needs the `gh` executable"),
        "plain resume must still check the retained workspace: {plain:?}"
    );
    assert_eq!(project_reads(&fixture), 1);

    let arguments = [
        "resume",
        &run_id,
        "--from-checkpoint",
        checkpoint,
        "--discard-workspace-changes",
    ];
    let certificate = fixture.root.path("missing-ca.pem");
    let untrusted = timeout(
        CLI_TIMEOUT,
        fixture
            .command(&path)
            .args(arguments)
            .env("SSL_CERT_FILE", &certificate)
            .output(),
    )
    .await
    .assert_value()
    .assert_value();
    assert!(
        !untrusted.status.success()
            && String::from_utf8_lossy(&untrusted.stderr)
                .contains(certificate.to_str().assert_value()),
        "checkpoint refusal must name the unreadable CA file: {untrusted:?}"
    );
    assert!(tools.join("gh").is_file(), "gh was not restored");
    assert_eq!(
        project_reads(&fixture),
        1,
        "unreadable CA must prevent the request"
    );
    assert_eq!(fs::read(&fixture.codex_log).assert_value(), original_turns);
    fs::remove_file(tools.join("gh")).assert_value();
    let rejected = timeout(
        CLI_TIMEOUT,
        fixture
            .command(&path)
            .args(arguments)
            .env("GH_TOKEN", "rejected-checkpoint-token")
            .env("HTTPS_PROXY", PROXY)
            .env("NO_PROXY", "caller-bypass.invalid")
            .output(),
    )
    .await
    .assert_value()
    .assert_value();
    let reason = String::from_utf8_lossy(&rejected.stderr);
    assert!(
        !rejected.status.success()
            && rejected.stdout.is_empty()
            && reason.contains("authenticated project/version read")
            && reason.contains("HTTP 401"),
        "restored gh must refuse authentication before agents start: {rejected:?}"
    );
    for secret in [
        PROXY,
        "checkpoint-proxy-secret",
        "rejected-checkpoint-token",
    ] {
        assert!(!reason.contains(secret), "secret in refusal: {reason}");
    }
    assert!(tools.join("gh").is_file(), "gh was not restored");
    assert_eq!(
        project_reads(&fixture),
        2,
        "one request per attempted resume"
    );
    assert_eq!(fs::read(&fixture.codex_log).assert_value(), original_turns);
    let status = json_output(&fixture, &["status", &run_id], &path).await;
    let recovery = status.assert_key("workspaceRecovery");
    assert_eq!(recovery.assert_key("recoverable"), true);
    assert!(recovery.get("successorRunId").is_none(), "{recovery}");
    let listed = json_output(&fixture, &["list"], &path).await;
    assert_eq!(listed.assert_key("runs").as_array().assert_value().len(), 1);

    // The refused restore retains its checkpoint and releases the claim for a corrected retry.
    fs::remove_file(tools.join("gh")).assert_value();
    let resumed = output(&fixture, &arguments, &path).await;
    assert!(resumed.status.success(), "{resumed:?}");
    let successor = receipt_id(&resumed);
    let watched = output(&fixture, &["watch", &successor], &path).await;
    assert!(
        !watched.status.success(),
        "fake delivery unexpectedly succeeded"
    );
    assert_eq!(
        project_reads(&fixture),
        3,
        "the controller duplicated preflight"
    );
    assert!(
        fs::read(&fixture.codex_log).assert_value().len() > original_turns.len(),
        "accepted checkpoint resume did not start agents"
    );
    assert!(tools.join("gh").is_file());
}

async fn output(fixture: &DeliveryFixture, arguments: &[&str], path: &OsString) -> Output {
    timeout(
        CLI_TIMEOUT,
        fixture
            .command(path)
            .args(arguments)
            .env("HTTPS_PROXY", PROXY)
            .env("NO_PROXY", "caller-bypass.invalid")
            .output(),
    )
    .await
    .assert_value()
    .assert_value()
}

async fn json_output(fixture: &DeliveryFixture, arguments: &[&str], path: &OsString) -> Value {
    let result = output(fixture, arguments, path).await;
    assert!(result.status.success(), "{result:?}");
    serde_json::from_slice(&result.stdout).assert_value()
}

fn receipt_id(result: &Output) -> String {
    let first = result
        .stdout
        .split(|byte| *byte == b'\n')
        .next()
        .assert_value();
    let receipt: Value = serde_json::from_slice(first).assert_value();
    receipt
        .assert_key("runId")
        .as_str()
        .assert_value()
        .to_owned()
}

fn project_reads(fixture: &DeliveryFixture) -> usize {
    fs::read_to_string(&fixture.gh_log)
        .assert_value()
        .lines()
        .filter(|line| *line == PROJECT_READ)
        .count()
}

fn write_checkpoint_gh(fixture: &DeliveryFixture, path: &Path) {
    openengine_cluster_testkit::fixture::write_executable(
        path,
        format!(
            r#"#!/bin/sh
if [ "$1" = api ] && [ "$2" = repos/acme/local-fixture ]; then
    printf '%s\n' '{PROJECT_READ}' >> '{log}'
    if [ -f '{marker}' ]; then
        if [ "$HTTPS_PROXY" != '{PROXY}' ] || [ "$NO_PROXY" != caller-bypass.invalid ]; then
            printf '%s\n' 'captured network settings missing' >&2
            exit 1
        fi
    fi
    if [ "$GH_TOKEN" != fixture-github-token ]; then
        printf 'HTTP 401: rejected checkpoint token %s\n' "$HTTPS_PROXY" >&2
        exit 1
    fi
    printf 'HTTP/2 200 OK\r\n\r\n{{"full_name":"acme/local-fixture"}}'
    exit 0
fi
printf '%s\n' 'fixture gh returns no GitHub data'
"#,
            log = fixture.gh_log.display(),
            marker = fixture.root.path("check-resume-network").display()
        ),
        0o755,
    )
    .assert_value();
}
