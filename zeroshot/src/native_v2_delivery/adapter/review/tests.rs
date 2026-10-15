use super::*;

#[test]
fn pending_ci_never_becomes_pull_request_ready() {
    assert!(matches!(
        ReviewProgress::from_open(SourceChecks::Pending, true, false),
        ReviewProgress::Pending
    ));
}

#[test]
fn passed_ci_can_be_ready_while_approval_is_missing() {
    assert!(matches!(
        ReviewProgress::from_open(SourceChecks::Passed, true, false),
        ReviewProgress::PullRequestReady
    ));
}

#[test]
fn passed_ci_without_the_approval_exception_is_mergeable() {
    assert!(matches!(
        ReviewProgress::from_open(SourceChecks::Passed, false, false),
        ReviewProgress::Mergeable
    ));
}
