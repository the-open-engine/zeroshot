use super::*;

#[test]
fn uncertain_head_recovery_never_silently_adopts_changed_work() {
    for (outcome, head_changed, expected_diagnostic) in [
        (
            GitHubReconciliationOutcome::Adopted,
            false,
            "without its mutation receipt",
        ),
        (
            GitHubReconciliationOutcome::Unchanged,
            true,
            "changed remote head",
        ),
    ] {
        let recovered = recovered_outcome(outcome, head_changed);
        let GitHubReconciliationOutcome::NeedsWork(diagnostic) = recovered else {
            panic!("uncertain recovery must require review: {recovered:?}");
        };
        assert!(diagnostic.contains(expected_diagnostic), "{diagnostic}");
    }

    for outcome in [
        GitHubReconciliationOutcome::Unchanged,
        GitHubReconciliationOutcome::NeedsWork("existing repair".to_owned()),
        GitHubReconciliationOutcome::Refused("existing refusal".to_owned()),
    ] {
        assert_eq!(recovered_outcome(outcome.clone(), false), outcome);
    }
}
