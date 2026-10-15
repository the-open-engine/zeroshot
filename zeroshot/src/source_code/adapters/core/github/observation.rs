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
    request: SourceDeliveryRead<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceDeliverySnapshot, SourceDeliveryError> {
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
    Ok(SourceDeliverySnapshot {
        review,
        head_revision,
    })
}

fn require_consistent_head(
    review: Option<&SourceReviewObservation>,
    head: Option<&str>,
) -> Result<(), SourceDeliveryError> {
    if let (Some(review), Some(head)) = (review, head) {
        if matches!(review.state, SourceReviewState::Open { .. }) && review.head_revision != head {
            return Err(SourceDeliveryError::Unavailable.with_context(format!(
                "GitHub PR/ref observations changed: PR head {}, ref head {head}",
                review.head_revision,
            )));
        }
    }
    Ok(())
}

async fn find_identity(
    authority: &GhCliDeliveryAuthority,
    request: SourceDeliveryRead<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Option<SourceReviewReceipt>, SourceDeliveryError> {
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
        return Err(SourceDeliveryError::identity(
            "multiple PRs match the run branch",
        ));
    }
    Ok(identities.pop())
}

async fn observe_review(
    authority: &GhCliDeliveryAuthority,
    request: SourceDeliveryRead<'_>,
    identity: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewObservation, SourceDeliveryError> {
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
        return Err(SourceDeliveryError::identity(format!(
            "expected PR {}; observed PR {}",
            identity.review_id, receipt.review_id,
        )));
    }
    let state = match (wire.state.as_str(), wire.merged, wire.merge_commit_sha) {
        ("closed", true, Some(revision)) if valid_revision(&revision) => {
            SourceReviewState::Merged {
                merge_revision: revision,
            }
        }
        ("closed", false, _) => SourceReviewState::Closed,
        ("open", false, _) => SourceReviewState::Open {
            checks: SourceChecks::Pending,
        },
        _ => return Err(SourceDeliveryError::Rejected),
    };
    Ok(receipt.observation(state))
}

async fn observe_ref(
    authority: &GhCliDeliveryAuthority,
    request: SourceDeliveryRead<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Option<String>, SourceDeliveryError> {
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

#[cfg(all(test, unix))]
pub(super) mod test_support {
    use super::*;

    pub(in crate::source_code_provider::adapters::core::github) const HEAD: &str =
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    pub(in crate::source_code_provider::adapters::core::github) const OTHER_HEAD: &str =
        "cccccccccccccccccccccccccccccccccccccccc";

    pub(in crate::source_code_provider::adapters::core::github) fn receipt() -> SourceReviewReceipt
    {
        SourceReviewReceipt {
            review_id: "17".to_owned(),
            repository: "acme/project".to_owned(),
            target_branch: "main".to_owned(),
            head_branch: "zeroshot/v2-test".to_owned(),
            head_revision: HEAD.to_owned(),
        }
    }

    pub(in crate::source_code_provider::adapters::core::github) fn assert_retryable_api(
        error: &SourceDeliveryError,
        context: &str,
    ) {
        assert!(matches!(error, SourceDeliveryError::Api(_)));
        assert!(error.retryable_operation());
        assert!(error.to_string().contains(context));
    }

    #[cfg(unix)]
    pub(in crate::source_code_provider::adapters::core::github) fn shell_literal(
        value: &str,
    ) -> String {
        assert!(!value.contains('\''));
        format!("'{value}'")
    }

    #[cfg(unix)]
    pub(in crate::source_code_provider::adapters::core::github) fn write_executable(
        program: &std::path::Path,
        source: String,
    ) {
        let result = openengine_cluster_testkit::fixture::write_executable(program, source, 0o700);
        assert!(result.is_ok(), "write test executable: {result:?}");
    }

    #[cfg(unix)]
    pub(in crate::source_code_provider::adapters::core::github) fn authority(
        program: std::path::PathBuf,
        home: &std::path::Path,
    ) -> GhCliDeliveryAuthority {
        GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
            gh_program: program,
            // Instrumented suites can briefly saturate process startup while running these
            // otherwise immediate local fixtures in parallel. Keep the production deadline out
            // of this test helper so a scheduler delay is not mistaken for an API failure.
            api_deadline: std::time::Duration::from_secs(30),
            ..GhCliAuthorityConfig::hosted(home.to_owned())
        })
    }
}

#[cfg(all(test, unix))]
#[path = "observation/tests.rs"]
mod tests;
