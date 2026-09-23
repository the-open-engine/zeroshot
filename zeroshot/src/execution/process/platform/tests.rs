use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[test]
fn cleanup_evidence_and_failures_preserve_the_full_decision_contract() {
    assert!(ProcessCleanupEvidence::NotRequired.proves_tree_empty());
    assert!(ProcessCleanupEvidence::Reaped.proves_tree_empty());
    assert!(!ProcessCleanupEvidence::TimedOut.proves_tree_empty());

    let error = io::Error::new(io::ErrorKind::PermissionDenied, "injected cleanup cause");
    let failure = cleanup_failure("process group inspection failed", &error);
    let detail = failure.error.assert_value();
    assert_eq!(failure.cleanup, ProcessCleanupEvidence::TimedOut);
    assert!(detail.starts_with("process group inspection failed"));
    assert!(detail.contains("kind=PermissionDenied"));
    assert!(detail.contains("raw_os_error=none"));
    assert!(detail.contains("message=injected cleanup cause"));

    let timeout = cleanup_timeout();
    assert_eq!(timeout.cleanup, ProcessCleanupEvidence::TimedOut);
    assert_eq!(timeout.error.as_deref(), Some("process cleanup timed out"));
    assert_eq!(join_errors(Vec::new()), None);
    assert_eq!(
        join_errors(vec!["kill cause".to_owned(), "wait cause".to_owned()]),
        Some("kill cause; wait cause".to_owned())
    );
}

#[cfg(target_os = "linux")]
#[test]
fn containment_exposes_only_the_authored_worker_identity_and_membership() {
    let cases = [
        (ProcessContainment::ProcessGroup, None, None),
        (
            ProcessContainment::WorkerUid { uid: 10, gid: 20 },
            Some((10, 20, None)),
            Some(WorkerMembership::Uid(10)),
        ),
        (
            ProcessContainment::WorkerGroup {
                uid: 10,
                gid: 20,
                group: 30,
            },
            Some((10, 20, Some(30))),
            Some(WorkerMembership::SupplementaryGroup(30)),
        ),
    ];
    for (containment, identity, membership) in cases {
        assert_eq!(containment.worker_identity(), identity);
        assert_eq!(containment.membership(), membership);
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn detached_and_absent_process_boundaries_settle_without_waiting() {
    let detached = ProcessTreeHandle {
        process_group_id: None,
        containment: ProcessContainment::ProcessGroup,
    };
    assert!(!detached.requires_explicit_cleanup_evidence());
    assert!(!process_tree_has_live_members(&detached).assert_value());
    let cleanup = await_group_exit(&detached, Instant::now()).await;
    assert_eq!(cleanup.cleanup, ProcessCleanupEvidence::Reaped);
    assert_eq!(cleanup.error, None);

    let termination =
        termination_without_status(&detached, Instant::now(), vec!["wait failed".to_owned()]).await;
    assert!(termination.exit_status.is_none());
    assert_eq!(termination.cleanup, ProcessCleanupEvidence::Reaped);
    assert_eq!(termination.error.as_deref(), Some("wait failed"));
    assert_eq!(
        cleanup_process_domain(ProcessContainment::ProcessGroup).await,
        ProcessCleanupEvidence::TimedOut
    );

    let absent_group = i32::MAX;
    let absent = ProcessTreeHandle {
        process_group_id: Some(absent_group),
        containment: ProcessContainment::ProcessGroup,
    };
    assert!(!process_tree_has_live_members(&absent).assert_value());
    assert_eq!(
        await_process_group_exit(absent_group, Instant::now())
            .await
            .cleanup,
        ProcessCleanupEvidence::Reaped
    );
    assert_eq!(
        await_worker_exit(WorkerMembership::Uid(u32::MAX), Instant::now())
            .await
            .cleanup,
        ProcessCleanupEvidence::Reaped
    );
}
