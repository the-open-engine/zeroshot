//! Required-check evidence reported alongside a conflicting pull request.
use super::*;

#[test]
fn conflicting_pull_request_reports_only_required_checks_that_started() {
    let conflict =
        |contexts| classify(policy_page("CONFLICTING", "DIRTY", contexts, (false, None)));
    for (contexts, expected) in [
        (None, SourceChecks::NotRequired),
        (
            Some(vec![check_run(
                "optional",
                "COMPLETED",
                Some("FAILURE"),
                false,
            )]),
            SourceChecks::NotRequired,
        ),
        (
            Some(vec![check_run("required", "IN_PROGRESS", None, true)]),
            SourceChecks::Pending,
        ),
        (
            Some(vec![check_run(
                "required",
                "COMPLETED",
                Some("SUCCESS"),
                true,
            )]),
            SourceChecks::Passed,
        ),
    ] {
        let snapshot = conflict(contexts);
        assert_eq!(snapshot.state, SourceReviewState::Conflict);
        assert_eq!(snapshot.conflict_checks, Some(expected));
    }
    let failed = conflict(Some(vec![
        check_run("required", "COMPLETED", Some("FAILURE"), true),
        check_run("still-running", "IN_PROGRESS", None, true),
    ]));
    assert!(matches!(
        failed.conflict_checks,
        Some(SourceChecks::Failed { .. })
    ));
    // Repair receives the same job details and log excerpts as a failure on an open review.
    assert_eq!(failed.failed_job_ids, vec![91]);
    let mut failed = failed;
    let before = failed.conflict_checks.clone();
    include_check_logs(
        &mut failed,
        &[(
            91,
            api::check_log_tail(
                b"setup passed\nAssertionError: conflicting head failed",
                SourceDeliveryCredential::token(""),
                &[],
            ),
        )],
    );
    assert_ne!(failed.conflict_checks, before);

    // A required context that never reported cannot start for a conflicting pull request.
    let mut page = policy_page(
        "CONFLICTING",
        "DIRTY",
        Some(vec![check_run(
            "pushed",
            "COMPLETED",
            Some("SUCCESS"),
            true,
        )]),
        (false, None),
    );
    page.pointer_mut(
        "/data/repository/pullRequest/baseRef/refUpdateRule/requiredStatusCheckContexts",
    )
    .assert_value()
    .clone_from(&json!(["pushed", "pull-request-only"]));
    assert_eq!(
        classify(page).conflict_checks,
        Some(SourceChecks::NotRequired)
    );

    let open = classify(policy_page("MERGEABLE", "CLEAN", None, (false, None)));
    assert_eq!(open.conflict_checks, None);
}
