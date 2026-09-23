use std::path::{Path, PathBuf};

use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};

use super::test_support::{
    HEAD, OTHER_HEAD, assert_retryable_api, authority, receipt, shell_literal, write_executable,
};
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

fn write_fixture(
    root: &Path,
    list: Value,
    observed_review: Value,
    reference_action: &str,
) -> PathBuf {
    let program = root.join("gh-observation-fixture");
    let source = format!(
        "#!/bin/sh\ncase \"$2\" in\n\
         repos/acme/project/pulls) printf '%s\\n' {} ;;\n\
         repos/acme/project/pulls/17) printf '%s\\n' {} ;;\n\
         repos/acme/project/git/ref/heads/zeroshot/v2-test) {} ;;\n\
         *) exit 19 ;;\n\
         esac\n",
        shell_literal(&list.to_string()),
        shell_literal(&observed_review.to_string()),
        reference_action,
    );
    write_executable(&program, source);
    program
}

struct ObservationOptions<'a> {
    known_review: Option<&'a GitHubReviewReceipt>,
    include_review: bool,
}

async fn observe_fixture(
    list: Value,
    observed_review: Value,
    reference_action: String,
    options: ObservationOptions<'_>,
) -> Result<GitHubDeliverySnapshot, GitHubAuthorityError> {
    let root = tempfile::tempdir().assert_value();
    let program = write_fixture(root.path(), list, observed_review, &reference_action);
    let authority = authority(program, root.path());
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

fn present_reference(head: &str) -> String {
    format!(
        "printf '%s\\n' {}",
        shell_literal(&reference(head).to_string())
    )
}

fn missing_reference() -> String {
    "printf '%s\\n' 'gh: Not Found (HTTP 404)' >&2; exit 1".to_owned()
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
