//! Read-only delivery discovery, including terminal PRs and a separately observed branch ref.
use serde::Deserialize;

use super::*;
use super::wire::{GitReferenceWire, read_review_receipt, reference_revision};

#[derive(Deserialize)]
struct ObservedReview {
    #[serde(flatten)]
    identity: PullRequestWire,
    state: String,
    merged: bool,
    merge_commit_sha: Option<String>,
}

pub(super) async fn observe(
    authority: &GhCliDeliveryAuthority,
    request: GitHubDeliveryRead<'_>,
    credential: GitHubCredential<'_>,
) -> Result<GitHubDeliverySnapshot, GitHubAuthorityError> {
    let identity = match (
        request.include_review,
        request
            .known_review
            .filter(|review| !review.review_id.is_empty()),
    ) {
        (false, _) => None,
        (true, Some(review)) => Some(review.clone()),
        (true, None) => find_identity(authority, request, credential).await?,
    };
    let review = match identity {
        Some(identity) => Some(observe_review(authority, request, &identity, credential).await?),
        None => None,
    };
    let head_revision = observe_ref(authority, request, credential).await?;
    require_consistent_head(review.as_ref(), head_revision.as_deref())?;
    Ok(GitHubDeliverySnapshot {
        review,
        head_revision,
    })
}

fn require_consistent_head(
    review: Option<&GitHubReviewObservation>,
    head: Option<&str>,
) -> Result<(), GitHubAuthorityError> {
    if let (Some(review), Some(head)) = (review, head) {
        if matches!(review.state, GitHubReviewState::Open { .. }) && review.head_revision != head {
            return Err(GitHubAuthorityError::Unavailable.with_context(format!(
                "GitHub PR/ref observations changed: PR head {}, ref head {head}",
                review.head_revision,
            )));
        }
    }
    Ok(())
}

async fn find_identity(
    authority: &GhCliDeliveryAuthority,
    request: GitHubDeliveryRead<'_>,
    credential: GitHubCredential<'_>,
) -> Result<Option<GitHubReviewReceipt>, GitHubAuthorityError> {
    let value = authority
        .api(
            &review_list_arguments(request.target, request.head_branch)?,
            credential,
        )
        .await?;
    let reviews: Vec<PullRequestWire> = super::api::decode_response(value, credential)?;
    let mut identities = reviews
        .into_iter()
        .map(|wire| read_review_receipt(wire, request.target, request.head_branch))
        .collect::<Result<Vec<_>, _>>()?;
    if identities.len() > 1 {
        return Err(GitHubAuthorityError::identity(
            "multiple PRs match the run branch",
        ));
    }
    Ok(identities.pop())
}

async fn observe_review(
    authority: &GhCliDeliveryAuthority,
    request: GitHubDeliveryRead<'_>,
    identity: &GitHubReviewReceipt,
    credential: GitHubCredential<'_>,
) -> Result<GitHubReviewObservation, GitHubAuthorityError> {
    let value = authority
        .api(
            &[
                format!(
                    "repos/{}/pulls/{}",
                    request.target.repository, identity.review_id
                ),
                "--method".to_owned(),
                "GET".to_owned(),
            ],
            credential,
        )
        .await?;
    let wire: ObservedReview = super::api::decode_response(value, credential)?;
    let receipt = read_review_receipt(wire.identity, request.target, request.head_branch)?;
    if receipt.review_id != identity.review_id {
        return Err(GitHubAuthorityError::identity(format!(
            "expected PR {}; observed PR {}",
            identity.review_id, receipt.review_id,
        )));
    }
    let state = match (wire.state.as_str(), wire.merged, wire.merge_commit_sha) {
        ("closed", true, Some(revision)) if valid_revision(&revision) => {
            GitHubReviewState::Merged {
                merge_revision: revision,
            }
        }
        ("closed", false, _) => GitHubReviewState::Closed,
        ("open", false, _) => GitHubReviewState::Open {
            checks: GitHubChecks::Pending,
        },
        _ => return Err(GitHubAuthorityError::Rejected),
    };
    Ok(receipt.observation(state))
}

async fn observe_ref(
    authority: &GhCliDeliveryAuthority,
    request: GitHubDeliveryRead<'_>,
    credential: GitHubCredential<'_>,
) -> Result<Option<String>, GitHubAuthorityError> {
    let result = authority
        .api(
            &[format!(
                "repos/{}/git/ref/heads/{}",
                request.target.repository, request.head_branch
            )],
            credential,
        )
        .await;
    let value = match result {
        Ok(value) => value,
        Err(error) if error.api_status() == Some(404) => return Ok(None),
        Err(error) => return Err(error),
    };
    let wire: GitReferenceWire = super::api::decode_response(value, credential)?;
    reference_revision(wire, request.head_branch).map(Some)
}

#[cfg(test)]
pub(super) mod test_support {
    use super::*;

    pub(in crate::native_v2_delivery::github) const HEAD: &str =
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    pub(in crate::native_v2_delivery::github) const OTHER_HEAD: &str =
        "cccccccccccccccccccccccccccccccccccccccc";

    pub(in crate::native_v2_delivery::github) fn receipt() -> GitHubReviewReceipt {
        GitHubReviewReceipt {
            review_id: "17".to_owned(),
            repository: "acme/project".to_owned(),
            target_branch: "main".to_owned(),
            head_branch: "zeroshot/v2-test".to_owned(),
            head_revision: HEAD.to_owned(),
        }
    }

    pub(in crate::native_v2_delivery::github) fn assert_retryable_api(
        error: &GitHubAuthorityError,
        context: &str,
    ) {
        assert!(matches!(error, GitHubAuthorityError::Api(_)));
        assert!(error.retryable_operation());
        assert!(error.to_string().contains(context));
    }

    #[cfg(unix)]
    pub(in crate::native_v2_delivery::github) fn shell_literal(value: &str) -> String {
        assert!(!value.contains('\''));
        format!("'{value}'")
    }

    #[cfg(unix)]
    pub(in crate::native_v2_delivery::github) fn write_executable(
        program: &std::path::Path,
        source: String,
    ) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(program, source).expect("fixture script must be writable");
        std::fs::set_permissions(program, std::fs::Permissions::from_mode(0o700))
            .expect("fixture script must be executable");
    }

    #[cfg(unix)]
    pub(in crate::native_v2_delivery::github) fn authority(
        program: std::path::PathBuf,
        home: &std::path::Path,
    ) -> GhCliDeliveryAuthority {
        GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
            gh_program: program,
            // Instrumented suites can briefly saturate process startup while running these
            // otherwise immediate local fixtures in parallel. Keep the production deadline out
            // of this test helper so a scheduler delay is not mistaken for an API failure.
            api_deadline: std::time::Duration::from_secs(10),
            ..GhCliAuthorityConfig::hosted(home.to_owned())
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
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
        assert!(matches!(duplicate, GitHubAuthorityError::Identity(_)));

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
}
