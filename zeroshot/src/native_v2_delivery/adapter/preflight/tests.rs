use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

const OBSERVED_HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const PUBLISHED_HEAD: &str = "cccccccccccccccccccccccccccccccccccccccc";
const INTENDED_HEAD: &str = "dddddddddddddddddddddddddddddddddddddddd";

fn receipt(head_revision: &str) -> GitHubReviewReceipt {
    GitHubReviewReceipt {
        review_id: "17".to_owned(),
        repository: "acme/project".to_owned(),
        target_branch: "main".to_owned(),
        head_branch: "zeroshot/v2-test".to_owned(),
        head_revision: head_revision.to_owned(),
    }
}

fn target() -> DeliveryTarget {
    DeliveryTarget::new(
        "acme/project",
        "main",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .assert_value()
}

fn observation() -> GitHubReviewObservation {
    GitHubReviewObservation {
        review_id: "17".to_owned(),
        repository: "acme/project".to_owned(),
        target_branch: "main".to_owned(),
        head_branch: "zeroshot/v2-test".to_owned(),
        head_revision: OBSERVED_HEAD.to_owned(),
        state: GitHubReviewState::Open {
            checks: GitHubChecks::Pending,
        },
        pull_request_ready: false,
        head_update_required: false,
    }
}

#[test]
fn reconciliation_anchor_requires_proof_and_uses_the_strongest_owned_revision() {
    let observed = receipt(OBSERVED_HEAD);
    let published = receipt(PUBLISHED_HEAD);
    let known = DeliveryState {
        published: Some(published.clone()),
        intended_push: Some(INTENDED_HEAD.to_owned()),
        ..DeliveryState::default()
    };
    assert_eq!(
        reconciliation_anchor(&known, &observed, true).assert_value(),
        published
    );

    let intended = DeliveryState {
        intended_push: Some(INTENDED_HEAD.to_owned()),
        ..DeliveryState::default()
    };
    let anchored = reconciliation_anchor(&intended, &observed, false).assert_value();
    assert_eq!(anchored.review_id, observed.review_id);
    assert_eq!(anchored.head_revision, INTENDED_HEAD);

    assert_eq!(
        reconciliation_anchor(&DeliveryState::default(), &observed, true).assert_value(),
        observed
    );
    let error = reconciliation_anchor(&DeliveryState::default(), &observed, false).assert_error();
    assert!(matches!(error, GitHubAuthorityError::Identity(_)));
    assert!(error.to_string().contains(OBSERVED_HEAD));
}

#[test]
fn observed_receipt_requires_a_branch_ref_and_preserves_review_identity() {
    let target = target();
    assert_eq!(
        observed_receipt(
            GitHubDeliverySnapshot {
                review: Some(observation()),
                head_revision: None,
            },
            &target,
            "zeroshot/v2-test",
        ),
        None
    );

    let reviewed = observed_receipt(
        GitHubDeliverySnapshot {
            review: Some(observation()),
            head_revision: Some(OBSERVED_HEAD.to_owned()),
        },
        &target,
        "zeroshot/v2-test",
    )
    .assert_value();
    assert_eq!(reviewed, receipt_from_observation(&observation()));

    let branch_only = observed_receipt(
        GitHubDeliverySnapshot {
            review: None,
            head_revision: Some(OBSERVED_HEAD.to_owned()),
        },
        &target,
        "zeroshot/v2-test",
    )
    .assert_value();
    assert!(branch_only.review_id.is_empty());
    assert_eq!(branch_only.repository, target.repository);
    assert_eq!(branch_only.target_branch, target.target_branch);
    assert_eq!(branch_only.head_branch, "zeroshot/v2-test");
    assert_eq!(branch_only.head_revision, OBSERVED_HEAD);
}
