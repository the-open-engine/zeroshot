use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::Value;

use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;

#[tokio::test]
async fn fetch_waits_for_automatic_maintenance() {
    let root = TestDirectory::new("git-maintenance");
    let program = root.write_executable(
        "git-trace",
        "#!/bin/sh\nexport GIT_TRACE2_EVENT=\"$0.trace\"\nexec /usr/bin/git \"$@\"\n",
    );
    let git = GitProcess {
        program: &program,
        token: None,
        uid: unsafe { libc::geteuid() },
        gid: unsafe { libc::getegid() },
        deadline: Instant::now() + GIT_TIMEOUT,
    };
    let seed = root.child("seed");
    let workspace = root.child("workspace");
    git.run(None, &["init".into(), seed.as_os_str().to_owned()])
        .await
        .assert_value_with("initialize source");
    git.run(
        Some(&seed),
        &[
            "-c".into(),
            "user.name=Fixture".into(),
            "-c".into(),
            "user.email=fixture@example.invalid".into(),
            "commit".into(),
            "--allow-empty".into(),
            "-m".into(),
            "fixture".into(),
        ],
    )
    .await
    .assert_value_with("commit source");
    let revision = git
        .capture(&seed, &["rev-parse".into(), "HEAD".into()])
        .await
        .assert_value_with("resolve revision");
    git.run(
        None,
        &[
            "clone".into(),
            "--no-tags".into(),
            "--no-checkout".into(),
            seed.as_os_str().to_owned(),
            workspace.as_os_str().to_owned(),
        ],
    )
    .await
    .assert_value_with("clone source");
    let branch = git
        .capture(
            &seed,
            &["symbolic-ref".into(), "--short".into(), "HEAD".into()],
        )
        .await
        .assert_value_with("resolve branch");
    let source = ResolvedSource {
        repository: openengine_cluster_protocol::SourceRepositoryId::new("owner/repository")
            .assert_value(),
        branch: openengine_cluster_protocol::SourceBranchId::new(branch.trim()).assert_value(),
        revision: openengine_cluster_protocol::SourceRevisionId::new(revision.trim())
            .assert_value(),
    };
    fetch_source(&git, &workspace, &source)
        .await
        .assert_value_with("fetch source branch and revision");
    checkout_revision(&git, &workspace, revision.trim())
        .await
        .assert_value_with("checkout revision");
    assert_eq!(
        git.capture(&workspace, &["rev-parse".into(), "HEAD".into()])
            .await
            .assert_value_with("checkout revision"),
        revision
    );

    let trace = root.read("git-trace.trace");
    let maintenance: Vec<Value> = trace
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).assert_value_with("Git trace event"))
        .filter(|event| {
            event["event"] == "child_start"
                && event["argv"]
                    .as_array()
                    .is_some_and(|args| args.iter().any(|arg| arg == "maintenance"))
        })
        .collect();
    assert!(
        !maintenance.is_empty(),
        "fetch must exercise automatic maintenance"
    );
    for event in maintenance {
        let args = event["argv"]
            .as_array()
            .assert_value_with("maintenance arguments");
        assert!(args.iter().any(|arg| arg == "--no-detach"), "{event}");
        assert!(!args.iter().any(|arg| arg == "--detach"), "{event}");
    }
}

#[test]
fn checkout_cleanup_rejects_nonempty_or_replaced_staging() {
    let root = TestDirectory::new("checkout-staging-identity");
    let workspace = root.child("workspace");
    let external = root.child("external");
    std::fs::create_dir(&workspace).assert_value();
    std::fs::create_dir(&external).assert_value();
    std::fs::write(external.join("keep"), "user-owned").assert_value();
    let identity = pristine_workspace(&workspace).assert_value();
    std::fs::write(workspace.join("existing"), "installed files").assert_value();
    assert!(pristine_workspace(&workspace).is_err());
    assert_eq!(root.read("workspace/existing"), "installed files");

    std::fs::rename(&workspace, root.child("original")).assert_value();
    std::os::unix::fs::symlink(&external, &workspace).assert_value();
    assert!(reset_workspace(&workspace, &identity, Instant::now() + GIT_TIMEOUT).is_err());
    assert_eq!(root.read("external/keep"), "user-owned");
}

#[tokio::test]
async fn checkout_commands_share_one_deadline_and_retain_timeout_output() {
    let root = TestDirectory::new("checkout-total-deadline");
    let program = root.write_executable(
        "git-slow",
        "#!/bin/sh\n/usr/bin/printf 'checkout progress\n' >&2\n/usr/bin/sleep 0.2\n",
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    let git = GitProcess {
        program: &program,
        token: None,
        uid: unsafe { libc::geteuid() },
        gid: unsafe { libc::getegid() },
        deadline,
    };
    git.run(None, &[]).await.assert_value();
    tokio::time::sleep_until(deadline - Duration::from_millis(100)).await;
    let error = git.run(None, &[]).await.assert_error();
    let RepositoryInstallError::Git(failure) = error else {
        panic!("expected captured Git timeout");
    };
    assert!(failure.stderr.contains("checkout progress"));
    assert!(failure.to_string().contains("timed out"));
    assert!(failure.stderr_truncated);
    let error = RepositoryInstallError::Recovery {
        failure: Box::new(RepositoryInstallError::Git(failure)),
        message: "staging directory was replaced".to_owned(),
    };
    let run_id = RunId::new("checkout-timeout-cleanup");
    let diagnostics = OperatorDiagnosticStore::default();
    error.record_diagnostic(&run_id, &diagnostics);
    let snapshot = diagnostics.snapshot(&run_id);
    assert!(snapshot.diagnostics[0].stderr.contains("checkout progress"));
    assert!(
        snapshot.diagnostics[0]
            .stderr
            .contains("staging directory was replaced")
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancelling_checkout_kills_git_and_its_helper_processes() {
    let root = TestDirectory::new("checkout-cancellation");
    let program = root.write_executable(
        "git-hanging",
        "#!/bin/sh\n/usr/bin/sleep 30 &\nchild=$!\n/usr/bin/printf '%s' \"$child\" > \"$0.pid\"\nwait\n",
    );
    let pid_path = root.child("git-hanging.pid");
    let capture = tokio::spawn(async move {
        GitProcess {
            program: &program,
            token: None,
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
            deadline: Instant::now() + GIT_TIMEOUT,
        }
        .run(None, &[])
        .await
    });
    let pid: u32 = poll_until(|| std::fs::read_to_string(&pid_path).ok()?.parse().ok()).await;
    capture.abort();
    assert!(capture.await.assert_error().is_cancelled());
    poll_until(|| {
        let running = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                stat.rsplit_once(") ")
                    .map(|(_, fields)| fields.starts_with('Z'))
            })
            .is_some_and(|zombie| !zombie);
        (!running).then_some(())
    })
    .await;
}

#[cfg(target_os = "linux")]
async fn poll_until<T>(mut read: impl FnMut() -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let Some(value) = read() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .assert_value_with("checkout process did not reach the expected state")
}

#[test]
fn cleanup_stops_at_the_original_deadline() {
    let root = TestDirectory::new("checkout-cleanup-deadline");
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    let identity = pristine_workspace(&workspace).assert_value();
    for index in 0..5000 {
        std::fs::write(workspace.join(index.to_string()), "partial checkout").assert_value();
    }
    let deadline = Instant::now() + Duration::from_millis(1);
    let error = reset_workspace(&workspace, &identity, deadline).assert_error();
    assert!(matches!(error, RepositoryInstallError::Deadline));
    assert!(
        std::fs::read_dir(&workspace)
            .assert_value()
            .next()
            .is_some()
    );
    assert!(identity.is_current(&workspace));
}
