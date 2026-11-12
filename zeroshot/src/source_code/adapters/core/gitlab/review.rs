use reqwest::Method;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use super::*;
use wire::{Discussion, MergeRequest, Project};

fn effect_key(repository: &str, branch: &str) -> String {
    format!("{repository}\0{branch}")
}

async fn find(
    authority: &GitLabDeliveryAuthority,
    request: SourceDeliveryRead<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Option<MergeRequest>, SourceDeliveryError> {
    let target = request.target;
    let branch = request.head_branch;
    let project = authority.project(&target.repository, credential).await?;
    let mut matches: Vec<MergeRequest> = authority
        .api
        .pages_request(
            ApiRequest::get(&["projects", &target.repository, "merge_requests"])
                .query("scope", "all")
                .query("state", "all")
                .query("source_branch", branch),
            credential,
        )
        .await?;
    if !request.include_terminal_reviews {
        matches.retain(|review| matches!(review.state.as_str(), "opened" | "locked"));
    }
    if matches.len() > 1 {
        return Err(super::api::identity_error(
            format!(
                "Multiple GitLab merge requests match source branch {branch} in project {} targeting {}; review IDs: {}. Resolve the ambiguous reviews before resuming.",
                target.repository,
                target.target_branch,
                matches
                    .iter()
                    .map(|value| format!("!{}", value.iid))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            credential,
        ));
    }
    let value = matches.pop();
    if let Some(value) = &value {
        let identity = SourceReviewReceipt {
            review_id: value.iid.to_string(),
            repository: target.repository.clone(),
            target_branch: target.target_branch.clone(),
            head_branch: branch.to_owned(),
            head_revision: value.sha.clone(),
        };
        value.require_review(&identity, project.id, credential)?;
    }
    Ok(value)
}

pub(super) async fn observe(
    authority: &GitLabDeliveryAuthority,
    request: SourceDeliveryRead<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceDeliverySnapshot, SourceDeliveryError> {
    let (observed, head_revision) = consistent_review_ref(authority, request, credential).await?;
    let review = match observed {
        Some(value) => {
            let project = authority
                .project(&request.target.repository, credential)
                .await?;
            let receipt = SourceReviewReceipt {
                review_id: value.iid.to_string(),
                repository: request.target.repository.clone(),
                target_branch: request.target.target_branch.clone(),
                head_branch: request.head_branch.to_owned(),
                head_revision: value.sha.clone(),
            };
            let mut observed = observation(&value, &receipt, &project)?;
            super::ci::include_failure_details(
                authority,
                super::ci::PipelineContext {
                    head: value.head_pipeline.as_ref(),
                    project: &project,
                },
                &mut observed,
                credential,
            )
            .await?;
            Some(observed)
        }
        None => None,
    };
    Ok(SourceDeliverySnapshot {
        review,
        head_revision,
    })
}

async fn consistent_review_ref(
    authority: &GitLabDeliveryAuthority,
    request: SourceDeliveryRead<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<(Option<MergeRequest>, Option<String>), SourceDeliveryError> {
    let mut rechecks = 2;
    loop {
        let observed = observed_review(authority, request, credential).await?;
        let head_revision = authority
            .branch_revision(&request.target.repository, request.head_branch, credential)
            .await?;
        if let Some(value) = &observed
            && let Err(error) = require_consistent_ref(
                value,
                head_revision.as_deref(),
                &request.target.repository,
                credential,
            )
        {
            if rechecks == 0 {
                return Err(error);
            }
            rechecks -= 1;
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            continue;
        }
        return Ok((observed, head_revision));
    }
}

async fn observed_review(
    authority: &GitLabDeliveryAuthority,
    request: SourceDeliveryRead<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Option<MergeRequest>, SourceDeliveryError> {
    if !request.include_review {
        return Ok(None);
    }
    if let Some(known) = request
        .known_review
        .filter(|value| !value.review_id.is_empty())
    {
        require_known_identity(request, known, credential)?;
        let value: MergeRequest = authority
            .api
            .json(
                ApiRequest::get(&[
                    "projects",
                    &known.repository,
                    "merge_requests",
                    &known.review_id,
                ]),
                credential,
            )
            .await?;
        let project = authority.project(&known.repository, credential).await?;
        let current = SourceReviewReceipt {
            head_revision: value.sha.clone(),
            ..known.clone()
        };
        value.require_review(&current, project.id, credential)?;
        Ok(Some(value))
    } else {
        find(authority, request, credential).await
    }
}

fn require_consistent_ref(
    value: &MergeRequest,
    revision: Option<&str>,
    repository: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    if matches!(value.state.as_str(), "opened" | "locked") && revision != Some(value.sha.as_str()) {
        return Err(super::api::identity_error(
            format!(
                "GitLab merge request {repository}!{} on source branch {} targeting {} still disagrees with its ref after rechecking: \
             merge request head {}; branch head {}",
                value.iid,
                value.source_branch,
                value.target_branch,
                value.sha,
                revision.unwrap_or("missing"),
            ),
            credential,
        ));
    }
    Ok(())
}

pub(super) fn normalized_title(title: &str) -> (String, bool) {
    let mut title = title.trim();
    while let Some(prefix) = ["Draft:", "[Draft]", "(Draft)"].into_iter().find(|prefix| {
        title
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    }) {
        title = title[prefix.len()..].trim_start();
    }
    let title = if title.is_empty() {
        "Zeroshot change"
    } else {
        title
    };
    let shortened = title.chars().count() > 255;
    let title: String = title
        .chars()
        .take(255)
        .collect::<String>()
        .trim_end()
        .to_owned();
    (title, shortened)
}

fn normalized_request(
    request: &SourceReviewRequest,
) -> Result<SourceReviewRequest, SourceDeliveryError> {
    let (title, shortened) = normalized_title(&request.title);
    let mut description = super::comment::normalize_body(&request.description);
    if shortened {
        // Rebuild this section from authored input on every delivery, never from the stored MR.
        let note = format!("Original generated title: {}", request.title);
        if !description.ends_with(&note) {
            description.push_str(&format!("\n\n{note}"));
        }
    }
    let description = super::comment::literal_body(&description);
    if !valid_delivery_description(&description) {
        return Err(SourceDeliveryError::Rejected);
    }
    Ok(SourceReviewRequest {
        title,
        description,
        ..request.clone()
    })
}

fn generated_description(request: &SourceReviewRequest) -> String {
    let issue = request
        .source_issue
        .as_ref()
        .map_or(String::new(), |issue| {
            format!("\n\nCloses #{}", issue.number)
        });
    format!(
        "{DELIVERY_GENERATED_BODY_START}\n{}{issue}\n{DELIVERY_GENERATED_BODY_END}",
        request.description
    )
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
            "GitLab review lacks its generated metadata section",
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
    let body = format!(
        "{}{}{}",
        &existing[..start],
        replacement,
        &existing[suffix..]
    );
    Ok(body)
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
    authority: &GitLabDeliveryAuthority,
    request: &SourceReviewRequest,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewReceipt, SourceDeliveryError> {
    let project = authority
        .project(&request.target.repository, credential)
        .await?;
    let request = normalized_request(request)?;
    let request = &request;
    let description = generated_description(request);
    let found = observed_review(authority, review_read(request), credential).await?;
    if let Some(found) = found {
        return update(authority, (request, &found, &description), credential).await;
    }
    create(authority, (request, &description, project.id), credential).await
}

async fn create(
    authority: &GitLabDeliveryAuthority,
    context: (&SourceReviewRequest, &str, u64),
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewReceipt, SourceDeliveryError> {
    let (request, description, project_id) = context;
    let revision = authority
        .branch_revision(&request.target.repository, &request.head_branch, credential)
        .await?;
    if revision.as_deref() != Some(&request.head_revision) {
        return Err(super::api::identity_error(
            format!(
                "GitLab review creation head is not the published candidate for project {}: source branch {} is at {}, expected {}; target branch {}",
                request.target.repository,
                request.head_branch,
                revision.as_deref().unwrap_or("missing"),
                request.head_revision,
                request.target.target_branch,
            ),
            credential,
        ));
    }
    let key = effect_key(&request.target.repository, &request.head_branch);
    begin_creation(authority, &key)?;
    let created = authority.api.json::<MergeRequest>(ApiRequest::write(&[
        "projects", &request.target.repository, "merge_requests",
    ], Method::POST, json!({
        "source_branch":request.head_branch, "target_branch":request.target.target_branch,
        "title":request.title, "description":description,
    })), credential).await;
    let result = match created {
        Ok(value) => Ok(value),
        Err(error) => reconcile_creation(authority, request, error, credential).await,
    };
    let value = result?;
    let receipt = value.receipt(request, project_id, credential)?;
    require_metadata(&value, &request.title, description, credential)?;
    authority
        .effects
        .lock()
        .map_err(|_| SourceDeliveryError::Unavailable)?
        .creates
        .remove(&key);
    Ok(receipt)
}

fn begin_creation(
    authority: &GitLabDeliveryAuthority,
    key: &str,
) -> Result<(), SourceDeliveryError> {
    if !authority
        .effects
        .lock()
        .map_err(|_| SourceDeliveryError::Unavailable)?
        .creates
        .insert(key.to_owned())
    {
        return Err(SourceDeliveryError::api(
            None,
            "GitLab review creation remains uncertain",
        ));
    }
    Ok(())
}

pub(super) fn missing_source_branch(body: &[u8]) -> bool {
    let Ok(Value::Object(mut body)) = serde_json::from_slice(body) else {
        return false;
    };
    if body.len() != 1 {
        return false;
    }
    let message_matches = |message: &str| {
        message.eq_ignore_ascii_case("source_branch does not exist")
            || message.eq_ignore_ascii_case("source branch does not exist")
    };
    match body.remove("message") {
        Some(Value::String(message)) => message_matches(&message),
        Some(Value::Array(messages)) => {
            !messages.is_empty()
                && messages
                    .iter()
                    .all(|message| message.as_str().is_some_and(message_matches))
        }
        Some(Value::Object(fields)) if fields.len() == 1 => fields
            .get("source_branch")
            .and_then(Value::as_array)
            .is_some_and(|messages| {
                !messages.is_empty()
                    && messages
                        .iter()
                        .all(|message| message.as_str() == Some("does not exist"))
            }),
        _ => false,
    }
}

async fn reconcile_creation(
    authority: &GitLabDeliveryAuthority,
    request: &SourceReviewRequest,
    original: SourceDeliveryError,
    credential: SourceDeliveryCredential<'_>,
) -> Result<MergeRequest, SourceDeliveryError> {
    match observed_review(authority, review_read(request), credential).await? {
        Some(value) => Ok(value),
        None if matches!(original.api_status(), Some(400..=499)) => {
            let key = effect_key(&request.target.repository, &request.head_branch);
            authority
                .effects
                .lock()
                .map_err(|_| SourceDeliveryError::Unavailable)?
                .creates
                .remove(&key);
            if original.api_status() == Some(400) && original.retryable_review_sync() {
                let context = super::api::display_text(
                    format!(
                        "GitLab merge request creation for branch {}",
                        request.head_branch
                    )
                    .into_bytes(),
                    false,
                    credential,
                );
                Err(original.with_context(context))
            } else {
                Err(original)
            }
        }
        None => Err(SourceDeliveryError::api(
            None,
            format!(
                "GitLab review creation response was lost and no exact review was observed\n{original}"
            ),
        )),
    }
}

async fn update(
    authority: &GitLabDeliveryAuthority,
    context: (&SourceReviewRequest, &MergeRequest, &str),
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewReceipt, SourceDeliveryError> {
    let (request, found, description) = context;
    let project = authority
        .project(&request.target.repository, credential)
        .await?;
    if found.state != "opened" {
        let next = match found.state.as_str() {
            "closed" => {
                "was closed. Resume is refused while it is closed; reopen it with the same source and target branches to resume, or start a new run"
            }
            "merged" => {
                "was already merged. Resume in merge mode requires a clean workspace at the exact reviewed head to confirm delivery; start a new run from the target branch for remaining work"
            }
            _ => {
                "is not open. Resume requires an open merge request with the same source and target branches"
            }
        };
        return Err(super::api::identity_error(
            format!(
                "GitLab merge request {}!{} (source branch {}, target branch {}, head revision {}) {next}",
                request.target.repository,
                found.iid,
                found.source_branch,
                found.target_branch,
                found.sha,
            ),
            credential,
        ));
    }
    let receipt = found.receipt(request, project.id, credential)?;
    let body = refresh_description(found.description.as_deref().unwrap_or(""), description)
        .map_err(|error| {
            super::api::delivery_context(error, merge_request_identity(&receipt), credential)
        })?;
    // A draft applied after creation belongs to its human author. Metadata refresh must not
    // mark it ready again, even when the generated description changes.
    let title = if found.draft {
        found
            .title
            .as_deref()
            .ok_or(SourceDeliveryError::Rejected)?
    } else {
        &request.title
    };
    if found.title.as_deref() == Some(title) && found.description.as_deref() == Some(&body) {
        return Ok(receipt);
    }
    let value: MergeRequest = authority
        .api
        .json(
            ApiRequest::write(
                &[
                    "projects",
                    &receipt.repository,
                    "merge_requests",
                    &receipt.review_id,
                ],
                Method::PUT,
                json!({"title":title, "description":body}),
            ),
            credential,
        )
        .await?;
    value.require_review(&receipt, project.id, credential)?;
    require_metadata(&value, title, &body, credential)?;
    Ok(receipt)
}

fn require_metadata(
    value: &MergeRequest,
    title: &str,
    description: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    if value.title.as_deref() != Some(title) || value.description.as_deref() != Some(description) {
        return Err(super::api::identity_error(
            format!(
                "GitLab did not confirm the generated review metadata for merge request !{} (source branch {}, target branch {}, head revision {})",
                value.iid, value.source_branch, value.target_branch, value.sha,
            ),
            credential,
        ));
    }
    Ok(())
}

pub(super) async fn inspect(
    authority: &GitLabDeliveryAuthority,
    receipt: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewObservation, SourceDeliveryError> {
    let project = authority.project(&receipt.repository, credential).await?;
    let value = authority.merge_request(receipt, credential).await?;
    let mut observed = observation(&value, receipt, &project)?;
    super::ci::include_failure_details(
        authority,
        super::ci::PipelineContext {
            head: value.head_pipeline.as_ref(),
            project: &project,
        },
        &mut observed,
        credential,
    )
    .await?;
    Ok(observed)
}

fn integrated_revision(value: &MergeRequest) -> Result<String, SourceDeliveryError> {
    let revision = value
        .merge_commit_sha
        .as_ref()
        .or(value.squash_commit_sha.as_ref());
    revision
        .filter(|revision| valid_git_revision(revision))
        .cloned()
        .ok_or_else(|| {
            SourceDeliveryError::api(None, "GitLab merged revision is not visible").temporary()
        })
}

fn observation(
    value: &MergeRequest,
    receipt: &SourceReviewReceipt,
    project: &Project,
) -> Result<SourceReviewObservation, SourceDeliveryError> {
    let state = match value.state.as_str() {
        "merged" => SourceReviewState::Merged {
            merge_revision: integrated_revision(value)?,
        },
        "closed" => SourceReviewState::Closed,
        "opened"
            if matches!(
                value.detailed_merge_status.as_str(),
                "conflict" | "need_rebase"
            ) =>
        {
            SourceReviewState::Conflict
        }
        "opened" | "locked" => SourceReviewState::Open {
            checks: checks(value, project)?,
        },
        _ => return Err(SourceDeliveryError::Rejected),
    };
    let handoff = approval_handoff(value, project);
    Ok(receipt.observation_with_readiness(state, handoff, false))
}

fn pipeline_checks(
    value: &MergeRequest,
    project: &Project,
) -> Result<SourceChecks, SourceDeliveryError> {
    if project.only_allow_merge_if_pipeline_succeeds == Some(false) {
        return Ok(SourceChecks::NotRequired);
    }
    let Some(pipeline) = &value.head_pipeline else {
        return Ok(SourceChecks::Pending);
    };
    if !pipeline_associated(pipeline, value, project)? {
        return Ok(SourceChecks::Pending);
    }
    Ok(match pipeline.status.as_str() {
        "success" => SourceChecks::Passed,
        "skipped" if project.allow_merge_on_skipped_pipeline == Some(true) => SourceChecks::Passed,
        "failed" => SourceChecks::Failed {
            diagnostic: format!("GitLab pipeline {} failed", pipeline.id),
        },
        _ => SourceChecks::Pending,
    })
}

fn pipeline_associated(
    pipeline: &wire::Pipeline,
    value: &MergeRequest,
    project: &Project,
) -> Result<bool, SourceDeliveryError> {
    if pipeline.id == 0
        || !valid_git_revision(&pipeline.sha)
        || pipeline.project_id.is_some_and(|id| id != project.id)
    {
        return Err(SourceDeliveryError::identity(
            "GitLab pipeline changed project or commit identity",
        ));
    }
    // Synthetic merged-results pipeline commits need separate parent/diff evidence.
    Ok(pipeline.sha == value.sha)
}

fn approval_handoff(value: &MergeRequest, project: &Project) -> bool {
    matches!(
        value.detailed_merge_status.as_str(),
        "not_approved" | "requested_changes"
    ) && !value.draft
        && !value.has_conflicts
        && discussions_ready(value, project)
        && project.merge_method == "merge"
        && project.merge_trains_enabled != Some(true)
        && matches!(
            pipeline_checks(value, project),
            Ok(SourceChecks::Passed | SourceChecks::NotRequired)
        )
}

fn checks(value: &MergeRequest, project: &Project) -> Result<SourceChecks, SourceDeliveryError> {
    let pipeline = pipeline_checks(value, project)?;
    if matches!(pipeline, SourceChecks::Failed { .. }) {
        return Ok(pipeline);
    }
    let ready = synchronous_merge_allowed(value, project)
        && (value.detailed_merge_status == "mergeable" || approval_handoff(value, project));
    Ok(if ready {
        pipeline
    } else {
        SourceChecks::Pending
    })
}

pub(super) async fn merge(
    authority: &GitLabDeliveryAuthority,
    receipt: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
    checkpoint: &dyn crate::source_code_provider::SourceReviewCheckpoint,
) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
    let project = authority.project(&receipt.repository, credential).await?;
    let current = authority.merge_request(receipt, credential).await?;
    if current.state == "merged" {
        integrated_revision(&current)?;
        return Ok(SourceMergeRequestOutcome::Accepted);
    }
    if matches!(
        current.detailed_merge_status.as_str(),
        "conflict" | "need_rebase"
    ) {
        return Ok(SourceMergeRequestOutcome::Conflict);
    }
    if current.detailed_merge_status != "mergeable"
        || !matches!(
            checks(&current, &project)?,
            SourceChecks::Passed | SourceChecks::NotRequired
        )
    {
        return Ok(SourceMergeRequestOutcome::Pending);
    }
    let key = effect_key(&receipt.repository, &receipt.review_id);
    let first_attempt = authority
        .effects
        .lock()
        .map_err(|_| SourceDeliveryError::Unavailable)?
        .merges
        .insert(key.clone());
    if !first_attempt {
        return Ok(SourceMergeRequestOutcome::Pending);
    }
    let result: Result<MergeRequest, _> = authority
        .api
        .json(
            ApiRequest::write(
                &[
                    "projects",
                    &receipt.repository,
                    "merge_requests",
                    &receipt.review_id,
                    "merge",
                ],
                Method::PUT,
                json!({"sha":receipt.head_revision,"auto_merge":false}),
            ),
            credential,
        )
        .await;
    let result = match result {
        Err(error) if error.api_status() == Some(401) => {
            // GitLab also uses 401 for a valid token without merge rights. Verify current
            // authentication separately; an expired/rejected token must still request refresh.
            let user = authority
                .api
                .json::<wire::Actor>(ApiRequest::get(&["user"]), credential)
                .await;
            let error = if matches!(user, Ok(user) if user.id > 0) {
                super::api::delivery_context(
                    error.permission_denied(),
                    format!(
                        "GitLab merge permission is missing for {}. Grant the token permission to merge into target branch {}, then resume.",
                        merge_request_identity(receipt),
                        receipt.target_branch,
                    ),
                    credential,
                )
            } else {
                error
            };
            Err(error)
        }
        result => result,
    };
    let rejection = result.as_ref().err().cloned();
    let settled = settle_merge(authority, (receipt, &project, &key), result, credential);
    if rejection
        .as_ref()
        .is_some_and(|error| matches!(error.api_status(), Some(405 | 409 | 422)))
    {
        let refreshed = authority.merge_request(receipt, credential).await?;
        if refreshed.detailed_merge_status == "discussions_not_resolved"
            && !feedback(authority, receipt, credential, checkpoint)
                .await?
                .conversations
                .is_empty()
        {
            return Ok(SourceMergeRequestOutcome::ReviewBlocked);
        }
    }
    settled
}

fn settle_merge(
    authority: &GitLabDeliveryAuthority,
    context: (&SourceReviewReceipt, &Project, &str),
    result: Result<MergeRequest, SourceDeliveryError>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
    let (receipt, project, key) = context;
    match result {
        Ok(value) => {
            value.require_review(receipt, project.id, credential)?;
            if value.state == "merged" {
                integrated_revision(&value)?;
                authority
                    .effects
                    .lock()
                    .map_err(|_| SourceDeliveryError::Unavailable)?
                    .merges
                    .remove(key);
            }
            Ok(SourceMergeRequestOutcome::Accepted)
        }
        Err(error) => {
            if matches!(error.api_status(), Some(400..=499)) {
                authority
                    .effects
                    .lock()
                    .map_err(|_| SourceDeliveryError::Unavailable)?
                    .merges
                    .remove(key);
            }
            Err(error)
        }
    }
}

pub(super) async fn withdraw(
    authority: &GitLabDeliveryAuthority,
    receipt: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceMergeWithdrawal, SourceDeliveryError> {
    let value = authority.merge_request(receipt, credential).await?;
    if value.state == "merged" {
        return Ok(SourceMergeWithdrawal::Merged {
            merge_revision: integrated_revision(&value)?,
        });
    }
    let key = effect_key(&receipt.repository, &receipt.review_id);
    if value.merge_when_pipeline_succeeds
        || value.state == "locked"
        || authority
            .effects
            .lock()
            .map_err(|_| SourceDeliveryError::Unavailable)?
            .merges
            .contains(&key)
    {
        return Err(SourceDeliveryError::api(
            None,
            super::api::display_text(format!(
                "GitLab merge settlement remains uncertain for {}; no repair handoff is authorized",
                merge_request_identity(receipt)
            ).into_bytes(), false, credential),
        ));
    }
    Ok(SourceMergeWithdrawal::Withdrawn)
}

pub(super) async fn feedback(
    authority: &GitLabDeliveryAuthority,
    receipt: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
    checkpoint: &dyn crate::source_code_provider::SourceReviewCheckpoint,
) -> Result<SourceReviewFeedback, SourceDeliveryError> {
    super::conversations::reconcile_resolutions(authority, receipt, credential, checkpoint).await?;
    feedback_snapshot(authority, receipt, credential).await
}

pub(super) async fn feedback_snapshot(
    authority: &GitLabDeliveryAuthority,
    receipt: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewFeedback, SourceDeliveryError> {
    let review = authority.merge_request(receipt, credential).await?;
    let discussions: Vec<Discussion> = authority
        .api
        .pages(
            &[
                "projects",
                &receipt.repository,
                "merge_requests",
                &receipt.review_id,
                "discussions",
            ],
            credential,
        )
        .await?;
    let project = authority.project(&receipt.repository, credential).await?;
    let mut items = Vec::new();
    let mut conversations = Vec::new();
    let mut permissions = BTreeMap::new();
    for discussion in discussions {
        if discussion.id.is_empty() {
            return Err(SourceDeliveryError::identity(
                "GitLab returned an empty discussion ID",
            ));
        }
        let unresolved = discussion
            .notes
            .iter()
            .any(|note| note.resolvable && note.resolved == Some(false));
        let resolvable = discussion.notes.iter().any(|note| note.resolvable);
        let version = super::conversations::version(&discussion.notes, None)?;
        let mut authorized = true;
        let mut authorized_notes = Vec::new();
        let mut body = String::new();
        for note in discussion.notes {
            super::comment::require_review_note_scope(&note, &review, project.id)?;
            if authority.published_replies.matches(
                receipt,
                &discussion.id,
                (note.id, &note.body),
            )? {
                continue;
            }
            if !note.system && !note.body.trim().is_empty() {
                let allowed = match permissions.get(&note.author.id) {
                    Some(allowed) => *allowed,
                    None => {
                        let allowed =
                            can_repair(authority, receipt, note.author.id, credential).await?;
                        permissions.insert(note.author.id, allowed);
                        allowed
                    }
                };
                authorized &= allowed;
                if unresolved && allowed {
                    authorized_notes.push(note.clone());
                    body.push_str(&format!(
                        "\nAuthor id={} note={}:\n{}\n",
                        note.author.id, note.id, note.body
                    ));
                } else if allowed && !resolvable {
                    items.push(feedback_item(&discussion.id, note));
                }
            }
        }
        if unresolved {
            if !body.trim().is_empty() {
                items.push(SourceReviewFeedbackItem {
                    key: format!("conversation:{}", discussion.id),
                    version: super::conversations::version(&authorized_notes, None)?,
                    author: "discussion participants".into(),
                    location: Some(discussion.id.clone()),
                    body,
                });
            }
            conversations.push(SourceReviewConversation {
                id: discussion.id,
                version,
                authorized,
            });
        }
    }
    authority.merge_request(receipt, credential).await?;
    items.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(SourceReviewFeedback {
        items,
        conversations,
    })
}

pub(super) async fn can_repair(
    authority: &GitLabDeliveryAuthority,
    receipt: &SourceReviewReceipt,
    author: u64,
    credential: SourceDeliveryCredential<'_>,
) -> Result<bool, SourceDeliveryError> {
    if author == 0 {
        return Err(SourceDeliveryError::identity(
            "GitLab feedback author is invalid",
        ));
    }
    // Include inherited and invited project membership, rather than direct members only.
    let member: wire::Member = match authority
        .api
        .json(
            ApiRequest::get(&[
                "projects",
                &receipt.repository,
                "members",
                "all",
                &author.to_string(),
            ]),
            credential,
        )
        .await
    {
        Ok(member) => member,
        Err(error) if error.api_status() == Some(404) => return Ok(false),
        Err(error) => return Err(error),
    };
    if member.id != author {
        return Err(SourceDeliveryError::identity(
            "GitLab feedback membership changed author identity",
        ));
    }
    let active_membership = match member.membership_state {
        wire::MembershipState::Active => true,
        wire::MembershipState::Awaiting => false,
        wire::MembershipState::Unreported => {
            // CE has no awaiting-membership feature or serialized membership state.
            // Its members/all endpoint excludes invitations and access requests. Do not
            // apply that response shape to EE or an instance with unknown edition.
            if authority.api.server_metadata(credential).await?.enterprise != Some(false) {
                return Err(SourceDeliveryError::api(
                    None,
                    "GitLab omitted membership state without confirming Community Edition",
                ));
            }
            true
        }
    };
    Ok(matches!(member.state, wire::AccountState::Active)
        && active_membership
        && member.access_level >= 30)
}

fn feedback_item(discussion: &str, note: wire::Note) -> SourceReviewFeedbackItem {
    let position = note.position.as_ref().map(Value::to_string);
    let digest = format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{}\0{}\0{:?}\0{:?}",
                note.updated_at, note.body, position, note.resolved
            )
            .as_bytes(),
        )
    );
    SourceReviewFeedbackItem {
        key: format!("discussion:{discussion}:note:{}", note.id),
        version: digest,
        author: note.author.id.to_string(),
        location: position,
        body: note.body,
    }
}

fn discussions_ready(value: &MergeRequest, project: &Project) -> bool {
    value.blocking_discussions_resolved == Some(true)
        || project.only_allow_merge_if_all_discussions_are_resolved == Some(false)
}

/// A clean merge still changes the candidate and must be checked by the worker again.
pub(super) async fn integrate_rebase(
    authority: &GitLabDeliveryAuthority,
    request: &SourceConflictRequest,
    target_revision: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceConflictOutcome, SourceDeliveryError> {
    let git = authority.git()?;
    let project = authority
        .project(&request.review.repository, credential)
        .await?;
    let outcome =
        transport::integrate_review_target(git, request, target_revision, credential).await?;
    match outcome {
        SourceReconciliationOutcome::NeedsWork(diagnostic) => {
            Err(SourceDeliveryError::repairable(diagnostic))
        }
        SourceReconciliationOutcome::Unchanged | SourceReconciliationOutcome::Adopted => {
            Err(super::api::identity_error(
                format!(
                    "{}: GitLab merge method {} requires rebasing onto {} revision {target_revision}, \
                 but the candidate already contains that revision; the merge request requires \
                 manual resolution",
                    merge_request_identity(&request.review),
                    project.merge_method,
                    request.review.target_branch,
                ),
                credential,
            ))
        }
        SourceReconciliationOutcome::Refused(diagnostic) => {
            Err(SourceDeliveryError::identity(diagnostic))
        }
    }
}

fn synchronous_merge_allowed(value: &MergeRequest, project: &Project) -> bool {
    value.state == "opened"
        && !value.draft
        && !value.has_conflicts
        && !value.merge_when_pipeline_succeeds
        && project.merge_trains_enabled != Some(true)
        && discussions_ready(value, project)
}

fn require_known_identity(
    request: SourceDeliveryRead<'_>,
    known: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    if known.repository != request.target.repository
        || known.target_branch != request.target.target_branch
        || known.head_branch != request.head_branch
    {
        return Err(super::api::identity_error(
            format!(
                "{} differs from the admitted project {}, source branch {}, target branch {}",
                merge_request_identity(known),
                request.target.repository,
                request.head_branch,
                request.target.target_branch,
            ),
            credential,
        ));
    }
    Ok(())
}

pub(super) fn merge_request_identity(receipt: &SourceReviewReceipt) -> String {
    format!(
        "merge request {}!{} (source branch {}, target branch {}, head revision {})",
        receipt.repository,
        receipt.review_id,
        wire::branch_identity(&receipt.head_branch),
        wire::branch_identity(&receipt.target_branch),
        wire::revision_identity(&receipt.head_revision),
    )
}
