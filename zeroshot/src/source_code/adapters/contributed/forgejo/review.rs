use std::time::Duration;

use reqwest::Method;
use serde_json::json;

use crate::source_code_provider::{
    DELIVERY_GENERATED_BODY_END, DELIVERY_GENERATED_BODY_START, SourceChecks,
    SourceDeliveryCredential, SourceDeliveryError, SourceDeliveryRead, SourceDeliverySnapshot,
    SourceReviewLinks, SourceReviewObservation, SourceReviewReceipt, SourceReviewRequest,
    SourceReviewState, valid_delivery_description, valid_git_revision,
};

use super::api::ApiRequest;
use super::authority::{ForgejoDeliveryAuthority, split_repository};
use super::ci::{FailureContext, include_failure_details};
use super::checks::{OpenReview, observe_open};
use super::wire::{self, PullRequest};

const MAX_TITLE_CHARS: usize = 255;
const DEFAULT_TITLE: &str = "Zeroshot change";
const WIP_PREFIXES: [&str; 2] = ["WIP:", "[WIP]"];
const REF_RECHECKS: usize = 2;
const REF_RECHECK_DELAY: Duration = Duration::from_secs(1);

pub(super) fn normalized_title(title: &str) -> String {
    let mut title = title.trim();
    while let Some(prefix) = WIP_PREFIXES.into_iter().find(|prefix| {
        title
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    }) {
        title = title[prefix.len()..].trim_start();
    }
    let title = if title.is_empty() {
        DEFAULT_TITLE
    } else {
        title
    };
    title
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// A link is built from the admitted instance, never from a value the server returned.
pub(super) fn links(base_url: &str, review: &SourceReviewReceipt) -> SourceReviewLinks {
    let base = base_url.strip_suffix('/').unwrap_or(base_url);
    let repository = &review.repository;
    let review_id = &review.review_id;
    let head_revision = &review.head_revision;
    SourceReviewLinks {
        url: format!("{base}/{repository}/pulls/{review_id}"),
        commit_url: format!("{base}/{repository}/commit/{head_revision}"),
    }
}

/// A pull request belongs to this delivery when its head branch and head repository agree. The
/// head branch is read from `label`, because `ref` names the branch only while it exists. A head
/// that belongs to another repository is a fork, so the search ignores it, and an empty label
/// names no branch at all. The target branch is validated afterwards, so a pull request that was
/// retargeted blocks a second one instead of being ignored.
async fn find(
    authority: &ForgejoDeliveryAuthority,
    request: SourceDeliveryRead<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Option<PullRequest>, SourceDeliveryError> {
    let (owner, name) = split_repository(&request.target.repository)?;
    let state = if request.include_terminal_reviews {
        "all"
    } else {
        "open"
    };
    let pulls: Vec<PullRequest> = authority
        .api
        .pages(
            ApiRequest::get(&["repos", owner, name, "pulls"]).query("state", state),
            credential,
        )
        .await?;
    let mut matches = pulls
        .into_iter()
        .filter(|pull| {
            pull.head.label == request.head_branch
                && pull
                    .head
                    .repo
                    .as_ref()
                    .is_some_and(|head| head.full_name == request.target.repository)
        })
        .collect::<Vec<PullRequest>>();
    if !request.include_terminal_reviews {
        matches.retain(|pull| pull.state == "open");
    }
    if matches.len() > 1 {
        return Err(SourceDeliveryError::identity(format!(
            "Multiple Forgejo pull requests match head branch {} in {}; review numbers: {}. Resolve the ambiguous pull requests before resuming.",
            wire::branch_identity(request.head_branch),
            wire::repository_identity(&request.target.repository),
            matches
                .iter()
                .map(|pull| pull.number.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        )));
    }
    let pull = matches.pop();
    if let Some(pull) = &pull {
        require_pull(
            pull,
            &request.target.repository,
            &request.target.target_branch,
            request.head_branch,
        )?;
    }
    Ok(pull)
}

pub(super) async fn observe(
    authority: &ForgejoDeliveryAuthority,
    request: SourceDeliveryRead<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceDeliverySnapshot, SourceDeliveryError> {
    authority.api.require_supported(credential).await?;
    let (pull, head_revision) = consistent_review_ref(authority, request, credential).await?;
    let review = match &pull {
        Some(pull) => Some(
            observation(
                authority,
                pull,
                &receipt_for(
                    pull,
                    &request.target.repository,
                    &request.target.target_branch,
                    request.head_branch,
                ),
                credential,
            )
            .await?,
        ),
        None => None,
    };
    Ok(SourceDeliverySnapshot {
        review,
        head_revision,
    })
}

async fn consistent_review_ref(
    authority: &ForgejoDeliveryAuthority,
    request: SourceDeliveryRead<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<(Option<PullRequest>, Option<String>), SourceDeliveryError> {
    let mut rechecks = REF_RECHECKS;
    loop {
        let observed = observed_review(authority, request, credential).await?;
        let head_revision = authority
            .branch_revision(&request.target.repository, request.head_branch, credential)
            .await?;
        if let Some(pull) = &observed
            && let Err(error) =
                require_consistent_ref(pull, head_revision.as_deref(), &request.target.repository)
        {
            if rechecks == 0 {
                return Err(error);
            }
            rechecks -= 1;
            tokio::time::sleep(REF_RECHECK_DELAY).await;
            continue;
        }
        return Ok((observed, head_revision));
    }
}

async fn observed_review(
    authority: &ForgejoDeliveryAuthority,
    request: SourceDeliveryRead<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Option<PullRequest>, SourceDeliveryError> {
    if !request.include_review {
        return Ok(None);
    }
    if let Some(known) = request
        .known_review
        .filter(|value| !value.review_id.is_empty())
    {
        require_known_identity(request, known)?;
        let (owner, name) = split_repository(&known.repository)?;
        let pull: PullRequest = authority
            .api
            .json(
                ApiRequest::get(&["repos", owner, name, "pulls", &known.review_id]),
                credential,
            )
            .await?;
        require_pull(
            &pull,
            &known.repository,
            &known.target_branch,
            &known.head_branch,
        )?;
        return Ok(Some(pull));
    }
    find(authority, request, credential).await
}

fn require_consistent_ref(
    pull: &PullRequest,
    revision: Option<&str>,
    repository: &str,
) -> Result<(), SourceDeliveryError> {
    if pull.state == "open" && revision != Some(&pull.head.sha) {
        return Err(SourceDeliveryError::identity(format!(
            "Forgejo pull request {} in {} on branch {} still disagrees with its ref after rechecking: pull head {}; branch head {}",
            pull.number,
            wire::repository_identity(repository),
            wire::branch_identity(&pull.head.label),
            wire::revision_identity(&pull.head.sha),
            revision
                .map(wire::revision_identity)
                .unwrap_or_else(|| "missing".to_owned()),
        )));
    }
    Ok(())
}

fn normalized_request(
    request: &SourceReviewRequest,
) -> Result<SourceReviewRequest, SourceDeliveryError> {
    let title = normalized_title(&request.title);
    let description = literal_body(&normalize_body(&request.description));
    if !valid_delivery_description(&description) {
        return Err(SourceDeliveryError::Rejected);
    }
    Ok(SourceReviewRequest {
        title,
        description,
        ..request.clone()
    })
}

fn generated_description(request: &SourceReviewRequest) -> Result<String, SourceDeliveryError> {
    if !valid_delivery_description(&request.description) {
        return Err(SourceDeliveryError::Rejected);
    }
    let issue = request
        .source_issue
        .as_ref()
        .map_or(String::new(), |issue| {
            format!("\n\nCloses #{}", issue.number)
        });
    Ok(format!(
        "{DELIVERY_GENERATED_BODY_START}\n{}{issue}\n{DELIVERY_GENERATED_BODY_END}",
        request.description
    ))
}

fn refresh_description(existing: &str, replacement: &str) -> Result<String, SourceDeliveryError> {
    if !existing.contains(DELIVERY_GENERATED_BODY_START)
        && !existing.contains(DELIVERY_GENERATED_BODY_END)
    {
        return Ok(if existing.is_empty() {
            replacement.to_owned()
        } else {
            format!("{existing}\n\n{replacement}")
        });
    }
    let Some(start) = existing.find(DELIVERY_GENERATED_BODY_START) else {
        return Err(SourceDeliveryError::identity(
            "Forgejo pull request lacks its generated metadata section",
        ));
    };
    let Some(end) = existing.find(DELIVERY_GENERATED_BODY_END) else {
        return Err(SourceDeliveryError::Rejected);
    };
    if start >= end
        || existing.matches(DELIVERY_GENERATED_BODY_START).count() != 1
        || existing.matches(DELIVERY_GENERATED_BODY_END).count() != 1
    {
        return Err(SourceDeliveryError::Rejected);
    }
    let suffix = end + DELIVERY_GENERATED_BODY_END.len();
    Ok(format!(
        "{}{}{}",
        &existing[..start],
        replacement,
        &existing[suffix..]
    ))
}

fn normalize_body(body: &str) -> String {
    body.replace('\r', "").trim_end().to_owned()
}

fn literal_body(body: &str) -> String {
    body.lines()
        .map(|line| {
            if let Some(line) = line.strip_prefix('/') {
                format!("\\/{line}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn review_read(request: &SourceReviewRequest) -> SourceDeliveryRead<'_> {
    SourceDeliveryRead {
        target: &request.target,
        head_branch: &request.head_branch,
        known_review: request.known_review.as_ref(),
        include_review: true,
        include_terminal_reviews: request.include_terminal_reviews,
    }
}

pub(super) async fn open_or_update(
    authority: &ForgejoDeliveryAuthority,
    request: &SourceReviewRequest,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewReceipt, SourceDeliveryError> {
    authority.api.require_supported(credential).await?;
    let request = normalized_request(request)?;
    let request = &request;
    let description = generated_description(request)?;
    let (found, _) = consistent_review_ref(authority, review_read(request), credential).await?;
    if let Some(found) = found {
        return update(authority, (request, &found, &description), credential).await;
    }
    create(authority, (request, &description), credential).await
}

async fn create(
    authority: &ForgejoDeliveryAuthority,
    context: (&SourceReviewRequest, &str),
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewReceipt, SourceDeliveryError> {
    let (request, description) = context;
    let repository = &request.target.repository;
    let revision = authority
        .branch_revision(repository, &request.head_branch, credential)
        .await?;
    if revision.as_deref() != Some(&request.head_revision) {
        return Err(SourceDeliveryError::identity(format!(
            "Forgejo pull request creation head is not the published candidate for {}: head branch {} is at {}, expected {}; base branch {}",
            wire::repository_identity(repository),
            wire::branch_identity(&request.head_branch),
            revision
                .as_deref()
                .map(wire::revision_identity)
                .unwrap_or_else(|| "missing".to_owned()),
            wire::revision_identity(&request.head_revision),
            wire::branch_identity(&request.target.target_branch),
        )));
    }
    let (owner, name) = split_repository(repository)?;
    let key = effect_key(repository, &request.head_branch);
    begin_creation(authority, &key)?;
    let created = authority
        .api
        .json::<PullRequest>(
            ApiRequest::write(
                &["repos", owner, name, "pulls"],
                Method::POST,
                json!({
                    "title": request.title,
                    "body": description,
                    "base": request.target.target_branch,
                    "head": request.head_branch,
                }),
            ),
            credential,
        )
        .await;
    let result = match created {
        Ok(pull) => Ok(pull),
        Err(error) => reconcile_creation(authority, request, error, credential).await,
    };
    let pull = result?;
    require_pull(
        &pull,
        repository,
        &request.target.target_branch,
        &request.head_branch,
    )?;
    let receipt = receipt_for(
        &pull,
        repository,
        &request.target.target_branch,
        &request.head_branch,
    );
    require_metadata(&pull, &request.title, description, &receipt)?;
    confirm_creation(authority, &key)?;
    Ok(receipt)
}

/// At-most-once creation: the key records the first attempt and is cleared only when that attempt
/// is confirmed, so an uncertain response can never lead to a second write.
fn effect_key(repository: &str, branch: &str) -> String {
    format!("{repository}\0{branch}")
}

fn begin_creation(
    authority: &ForgejoDeliveryAuthority,
    key: &str,
) -> Result<(), SourceDeliveryError> {
    if !authority
        .effects
        .lock()
        .map_err(|_| SourceDeliveryError::Unavailable)?
        .insert(key.to_owned())
    {
        return Err(SourceDeliveryError::api(
            None,
            "Forgejo review creation remains uncertain",
        ));
    }
    Ok(())
}

fn confirm_creation(
    authority: &ForgejoDeliveryAuthority,
    key: &str,
) -> Result<(), SourceDeliveryError> {
    authority
        .effects
        .lock()
        .map_err(|_| SourceDeliveryError::Unavailable)?
        .remove(key);
    Ok(())
}

/// A failed creation is reconciled by reading again, never by sending the write twice. A pull
/// carrying the delivered revision is adopted; a definite refusal releases the record, because a
/// refused write cannot have been applied twice, while an uncertain one keeps it.
async fn reconcile_creation(
    authority: &ForgejoDeliveryAuthority,
    request: &SourceReviewRequest,
    original: SourceDeliveryError,
    credential: SourceDeliveryCredential<'_>,
) -> Result<PullRequest, SourceDeliveryError> {
    match observed_review(authority, review_read(request), credential).await? {
        Some(pull) if pull.head.sha == request.head_revision => Ok(pull),
        Some(_) => Err(original),
        None if matches!(original.api_status(), Some(400..=499)) => {
            let key = effect_key(&request.target.repository, &request.head_branch);
            authority
                .effects
                .lock()
                .map_err(|_| SourceDeliveryError::Unavailable)?
                .remove(&key);
            Err(original)
        }
        None => Err(SourceDeliveryError::api(
            None,
            format!(
                "Forgejo review creation response was lost and no exact review was observed\n{original}"
            ),
        )),
    }
}

async fn update(
    authority: &ForgejoDeliveryAuthority,
    context: (&SourceReviewRequest, &PullRequest, &str),
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewReceipt, SourceDeliveryError> {
    let (request, found, description) = context;
    let repository = &request.target.repository;
    if found.merged || found.state != "open" {
        let next = match found.state.as_str() {
            "closed" if found.merged => {
                "was already merged. Resume in merge mode requires a clean workspace at the exact reviewed head to confirm delivery; start a new run from the target branch for remaining work"
            }
            "closed" => {
                "was closed. Resume is refused while it is closed; reopen it with the same head and base branches to resume, or start a new run"
            }
            _ => {
                "is not open. Resume requires an open pull request with the same head and base branches"
            }
        };
        return Err(SourceDeliveryError::identity(format!(
            "Forgejo pull request {} in {} (head branch {}, base branch {}, head revision {}) {next}",
            found.number,
            wire::repository_identity(repository),
            wire::branch_identity(&found.head.label),
            wire::branch_identity(&found.base.reference),
            wire::revision_identity(&found.head.sha),
        )));
    }
    require_pull(
        found,
        repository,
        &request.target.target_branch,
        &request.head_branch,
    )?;
    if found.head.sha != request.head_revision {
        return Err(SourceDeliveryError::identity(format!(
            "Forgejo pull request {} in {} (head branch {}, base branch {}, head revision {}) does not carry the delivered revision {}",
            found.number,
            wire::repository_identity(repository),
            wire::branch_identity(&found.head.label),
            wire::branch_identity(&found.base.reference),
            wire::revision_identity(&found.head.sha),
            wire::revision_identity(&request.head_revision),
        )));
    }
    let receipt = receipt_for(
        found,
        repository,
        &request.target.target_branch,
        &request.head_branch,
    );
    let body = refresh_description(found.body.as_deref().unwrap_or(""), description)?;
    // A draft applied after creation belongs to its human author. Metadata refresh must not
    // mark it ready again, even when the generated description changes.
    let title = if found.draft {
        if found.title.is_empty() {
            return Err(SourceDeliveryError::Rejected);
        }
        &found.title
    } else {
        &request.title
    };
    if title == &found.title && found.body.as_deref() == Some(&body) {
        return Ok(receipt);
    }
    let (owner, name) = split_repository(repository)?;
    let pull: PullRequest = authority
        .api
        .json(
            ApiRequest::write(
                &["repos", owner, name, "pulls", &receipt.review_id],
                Method::PATCH,
                json!({"title": title, "body": body}),
            ),
            credential,
        )
        .await?;
    require_pull(
        &pull,
        repository,
        &request.target.target_branch,
        &request.head_branch,
    )?;
    require_metadata(&pull, title, &body, &receipt)?;
    Ok(receipt)
}

fn require_metadata(
    pull: &PullRequest,
    title: &str,
    description: &str,
    receipt: &SourceReviewReceipt,
) -> Result<(), SourceDeliveryError> {
    if pull.title != title || pull.body.as_deref() != Some(description) {
        return Err(SourceDeliveryError::identity(format!(
            "Forgejo did not confirm the generated pull request metadata for {} (head branch {}, base branch {}, head revision {})",
            review_identity(receipt),
            wire::branch_identity(&pull.head.label),
            wire::branch_identity(&pull.base.reference),
            wire::revision_identity(&pull.head.sha),
        )));
    }
    Ok(())
}

pub(super) async fn inspect(
    authority: &ForgejoDeliveryAuthority,
    receipt: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewObservation, SourceDeliveryError> {
    authority.api.require_supported(credential).await?;
    let (owner, name) = split_repository(&receipt.repository)?;
    let pull: PullRequest = authority
        .api
        .json(
            ApiRequest::get(&["repos", owner, name, "pulls", &receipt.review_id]),
            credential,
        )
        .await?;
    require_pull(
        &pull,
        &receipt.repository,
        &receipt.target_branch,
        &receipt.head_branch,
    )?;
    require_bound_head(&pull, receipt)?;
    observation(authority, &pull, receipt, credential).await
}

async fn observation(
    authority: &ForgejoDeliveryAuthority,
    pull: &PullRequest,
    receipt: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewObservation, SourceDeliveryError> {
    let observed = review_state(authority, &receipt.repository, pull, credential).await?;
    // A merged or closed pull request never waits for a person, and an open one is ready only when
    // its checks have settled and an approval is still missing.
    Ok(receipt.observation_with_readiness(observed.state, observed.pull_request_ready, false))
}

pub(super) fn merged_revision(pull: &PullRequest) -> Result<String, SourceDeliveryError> {
    match pull.merge_commit_sha.as_deref() {
        Some(sha) if valid_git_revision(sha) => Ok(sha.to_owned()),
        _ => Err(SourceDeliveryError::api(
            None,
            format!(
                "Forgejo pull request {} is merged without a merge commit revision",
                pull.number
            ),
        )
        .temporary()),
    }
}

async fn review_state(
    authority: &ForgejoDeliveryAuthority,
    repository: &str,
    pull: &PullRequest,
    credential: SourceDeliveryCredential<'_>,
) -> Result<OpenReview, SourceDeliveryError> {
    if pull.merged {
        let revision = merged_revision(pull)?;
        return Ok(OpenReview::new(
            SourceReviewState::Merged {
                merge_revision: revision,
            },
            false,
        ));
    }
    match pull.state.as_str() {
        "closed" => Ok(OpenReview::new(SourceReviewState::Closed, false)),
        "open" => {
            let mut observed = observe_open(authority, repository, pull, credential).await?;
            let SourceReviewState::Open { checks } = &mut observed.state else {
                return Ok(observed);
            };
            // Only a check that failed has job evidence, so a pull request whose checks did not
            // fail reads nothing.
            if !matches!(checks, SourceChecks::Failed { .. }) {
                return Ok(observed);
            }
            let context = FailureContext {
                repository,
                head_revision: &pull.head.sha,
            };
            include_failure_details(&authority.api, checks, &context, credential).await?;
            Ok(observed)
        }
        _ => Err(SourceDeliveryError::Rejected),
    }
}

pub(super) fn require_pull(
    pull: &PullRequest,
    repository: &str,
    target_branch: &str,
    head_branch: &str,
) -> Result<(), SourceDeliveryError> {
    if pull.base.reference != target_branch {
        return Err(SourceDeliveryError::identity(format!(
            "Forgejo pull request {} targets {} instead of {}",
            pull.number,
            wire::branch_identity(&pull.base.reference),
            wire::branch_identity(target_branch),
        )));
    }
    let head = pull.head.repo.as_ref().ok_or_else(|| {
        SourceDeliveryError::identity(format!(
            "Forgejo pull request {} returned no head repository",
            pull.number
        ))
    })?;
    if head.full_name != repository {
        return Err(SourceDeliveryError::identity(format!(
            "Forgejo pull request {} head belongs to {} instead of {}",
            pull.number,
            wire::repository_identity(&head.full_name),
            wire::repository_identity(repository),
        )));
    }
    if pull.head.label != head_branch {
        return Err(SourceDeliveryError::identity(format!(
            "Forgejo pull request {} is on branch {} instead of {}",
            pull.number,
            wire::branch_identity(&pull.head.label),
            wire::branch_identity(head_branch),
        )));
    }
    if !valid_git_revision(&pull.head.sha) {
        return Err(SourceDeliveryError::identity(format!(
            "Forgejo pull request {} returned {} as its head revision",
            pull.number,
            wire::revision_identity(&pull.head.sha),
        )));
    }
    Ok(())
}

/// The head the observation and a merge are decided for is the one the delivery bound to its
/// receipt. An open pull request that now carries another head, or a merged pull request whose
/// recorded head is another revision, is not the review the receipt names. A closed, unmerged
/// pull request is not checked.
pub(super) fn require_bound_head(
    pull: &PullRequest,
    receipt: &SourceReviewReceipt,
) -> Result<(), SourceDeliveryError> {
    if pull.merged && pull.head.sha != receipt.head_revision {
        return Err(SourceDeliveryError::identity(format!(
            "Forgejo pull request {} was merged at head revision {} instead of the reviewed head {}. Inspect the merge before resuming.",
            pull.number,
            wire::revision_identity(&pull.head.sha),
            wire::revision_identity(&receipt.head_revision),
        )));
    }
    if pull.merged || pull.state != "open" {
        return Ok(());
    }
    if pull.head.sha != receipt.head_revision {
        return Err(SourceDeliveryError::identity(format!(
            "Forgejo pull request {} changed the bound review head: head revision {} instead of {}. Restore the bound review identity before resuming.",
            pull.number,
            wire::revision_identity(&pull.head.sha),
            wire::revision_identity(&receipt.head_revision),
        )));
    }
    Ok(())
}

fn require_known_identity(
    request: SourceDeliveryRead<'_>,
    known: &SourceReviewReceipt,
) -> Result<(), SourceDeliveryError> {
    if known.repository != request.target.repository
        || known.target_branch != request.target.target_branch
        || known.head_branch != request.head_branch
    {
        return Err(SourceDeliveryError::identity(format!(
            "{} differs from the admitted repository {}, head branch {}, base branch {}",
            review_identity(known),
            wire::repository_identity(&request.target.repository),
            wire::branch_identity(request.head_branch),
            wire::branch_identity(&request.target.target_branch),
        )));
    }
    Ok(())
}

fn receipt_for(
    pull: &PullRequest,
    repository: &str,
    target_branch: &str,
    head_branch: &str,
) -> SourceReviewReceipt {
    SourceReviewReceipt {
        review_id: pull.number.to_string(),
        repository: repository.to_owned(),
        target_branch: target_branch.to_owned(),
        head_branch: head_branch.to_owned(),
        head_revision: pull.head.sha.clone(),
    }
}

fn review_identity(receipt: &SourceReviewReceipt) -> String {
    format!(
        "pull request {} (head branch {}, base branch {}, head revision {})",
        receipt.review_id,
        wire::branch_identity(&receipt.head_branch),
        wire::branch_identity(&receipt.target_branch),
        wire::revision_identity(&receipt.head_revision),
    )
}
