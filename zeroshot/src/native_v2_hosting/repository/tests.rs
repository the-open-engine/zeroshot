use openengine_cluster_testkit::assertions::AssertValue;
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
