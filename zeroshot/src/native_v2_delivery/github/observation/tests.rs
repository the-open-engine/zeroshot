use std::path::{Path, PathBuf};

use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};

use super::test_support::{HEAD, OTHER_HEAD, assert_retryable_api, authority, receipt};
use super::*;
use crate::native_v2_delivery::DeliveryTarget;

const MERGE: &str = "dddddddddddddddddddddddddddddddddddddddd";

fn target() -> DeliveryTarget {
    DeliveryTarget::new(
        "acme/project",
        "main",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .assert_value()
}

fn review(state: &str, merged: bool, merge_revision: Option<&str>, head: &str) -> Value {
    json!({
        "number": 17,
        "state": state,
        "merged": merged,
        "merge_commit_sha": merge_revision,
        "base": {
            "ref": "main",
            "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "repo": {"full_name": "acme/project"}
        },
        "head": {
            "ref": "zeroshot/v2-test",
            "sha": head,
            "repo": {"full_name": "acme/project"}
        }
    })
}

fn reference(head: &str) -> Value {
    json!({
        "ref": "refs/heads/zeroshot/v2-test",
        "object": {"sha": head, "type": "commit"}
    })
}

enum FixtureReference {
    Present(Value),
    Missing,
}

fn write_fixture(root: &Path, list: Value, observed_review: Value, reference: FixtureReference) {
    std::fs::write(root.join("reviews.json"), list.to_string()).assert_value();
    std::fs::write(root.join("review.json"), observed_review.to_string()).assert_value();
    if let FixtureReference::Present(reference) = reference {
        std::fs::write(root.join("reference.json"), reference.to_string()).assert_value();
    }
}

fn fixture_program() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/native_v2_delivery/github/observation/gh-fixture.sh")
}

struct ObservationOptions<'a> {
    known_review: Option<&'a GitHubReviewReceipt>,
    include_review: bool,
}

async fn observe_fixture(
    list: Value,
    observed_review: Value,
    reference: FixtureReference,
    options: ObservationOptions<'_>,
) -> Result<GitHubDeliverySnapshot, GitHubAuthorityError> {
    let root = tempfile::tempdir().assert_value();
    write_fixture(root.path(), list, observed_review, reference);
    let authority = authority(fixture_program(), root.path());
    let target = target();
    observe(
        &authority,
        GitHubDeliveryRead {
            target: &target,
            head_branch: "zeroshot/v2-test",
            known_review: options.known_review,
            include_review: options.include_review,
        },
        GitHubCredential("test-token"),
    )
    .await
}

fn present_reference(head: &str) -> FixtureReference {
    FixtureReference::Present(reference(head))
}

fn missing_reference() -> FixtureReference {
    FixtureReference::Missing
}

#[tokio::test]
async fn observation_discovers_optional_review_and_preserves_terminal_states() {
    let open = review("open", false, None, HEAD);
    let snapshot = observe_fixture(
        json!([]),
        open.clone(),
        present_reference(HEAD),
        ObservationOptions {
            known_review: None,
            include_review: true,
        },
    )
    .await
    .assert_value();
    assert_eq!(snapshot.review, None);
    assert_eq!(snapshot.head_revision.as_deref(), Some(HEAD));

    let snapshot = observe_fixture(
        json!([open.clone()]),
        open,
        present_reference(HEAD),
        ObservationOptions {
            known_review: None,
            include_review: true,
        },
    )
    .await
    .assert_value();
    assert_eq!(
        snapshot.review.as_ref().map(|review| &review.state),
        Some(&GitHubReviewState::Open {
            checks: GitHubChecks::Pending
        })
    );

    for (wire, expected) in [
        (
            review("closed", true, Some(MERGE), HEAD),
            GitHubReviewState::Merged {
                merge_revision: MERGE.to_owned(),
            },
        ),
        (
            review("closed", false, None, HEAD),
            GitHubReviewState::Closed,
        ),
    ] {
        let known = receipt();
        let snapshot = observe_fixture(
            json!([]),
            wire,
            missing_reference(),
            ObservationOptions {
                known_review: Some(&known),
                include_review: true,
            },
        )
        .await
        .assert_value();
        assert_eq!(
            snapshot.review.as_ref().map(|review| &review.state),
            Some(&expected)
        );
        assert_eq!(snapshot.head_revision, None);
    }
}

#[tokio::test]
async fn observation_fails_closed_on_ambiguous_or_changing_authority() {
    let open = review("open", false, None, HEAD);
    let duplicate = observe_fixture(
        json!([open.clone(), open.clone()]),
        open.clone(),
        present_reference(HEAD),
        ObservationOptions {
            known_review: None,
            include_review: true,
        },
    )
    .await
    .assert_error();
    assert!(
        matches!(&duplicate, GitHubAuthorityError::Identity(_)),
        "duplicate reviews must fail with an identity error, got {duplicate:?}"
    );

    let known = receipt();
    let changed = observe_fixture(
        json!([]),
        open.clone(),
        present_reference(OTHER_HEAD),
        ObservationOptions {
            known_review: Some(&known),
            include_review: true,
        },
    )
    .await
    .assert_error();
    assert_retryable_api(&changed, "PR/ref observations changed");

    for invalid in [
        review("closed", true, None, HEAD),
        review("open", true, Some(MERGE), HEAD),
        review("closed", true, Some("not-a-revision"), HEAD),
    ] {
        let error = observe_fixture(
            json!([]),
            invalid,
            present_reference(HEAD),
            ObservationOptions {
                known_review: Some(&known),
                include_review: true,
            },
        )
        .await
        .assert_error();
        assert_eq!(error, GitHubAuthorityError::Rejected);
    }

    let without_review = observe_fixture(
        json!([open.clone(), open]),
        review("invalid", true, None, "invalid"),
        present_reference(HEAD),
        ObservationOptions {
            known_review: Some(&known),
            include_review: false,
        },
    )
    .await
    .assert_value();
    assert_eq!(without_review.review, None);
    assert_eq!(without_review.head_revision.as_deref(), Some(HEAD));
}
