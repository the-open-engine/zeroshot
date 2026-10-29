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
    target: &DeliveryTarget,
    branch: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Option<MergeRequest>, SourceDeliveryError> {
    let project = authority.project(&target.repository, credential).await?;
    let mut matches: Vec<MergeRequest> = authority
        .api
        .json(
            ApiRequest::get(&["projects", &target.repository, "merge_requests"])
                .query("scope", "all")
                .query("state", "all")
                .query("source_branch", branch)
                .query("target_branch", &target.target_branch)
                .query("per_page", "100"),
            credential,
        )
        .await?;
    if matches.len() > 1 {
        return Err(SourceDeliveryError::identity(
            "Multiple GitLab merge requests match the run branch",
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
        value.require_review(&identity, project.id)?;
    }
    Ok(value)
}

pub(super) async fn observe(
    authority: &GitLabDeliveryAuthority,
    request: SourceDeliveryRead<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceDeliverySnapshot, SourceDeliveryError> {
    let observed = observed_review(authority, request, credential).await?;
    let head_revision = authority
        .branch_revision(&request.target.repository, request.head_branch, credential)
        .await?;
    let review = match observed {
        Some(value) => {
            require_consistent_ref(&value, head_revision.as_deref())?;
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
            Some(observation(&value, &receipt, &project)?)
        }
        None => None,
    };
    Ok(SourceDeliverySnapshot {
        review,
        head_revision,
    })
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
        require_known_identity(request, known)?;
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
        value.require_review(&current, project.id)?;
        Ok(Some(value))
    } else {
        find(authority, request.target, request.head_branch, credential).await
    }
}

fn require_consistent_ref(
    value: &MergeRequest,
    revision: Option<&str>,
) -> Result<(), SourceDeliveryError> {
    if matches!(value.state.as_str(), "opened" | "locked") && revision != Some(value.sha.as_str()) {
        return Err(SourceDeliveryError::api(
            None,
            "GitLab merge request/ref observations changed",
        )
        .temporary());
    }
    Ok(())
}

fn generated_description(request: &SourceReviewRequest) -> Result<String, SourceDeliveryError> {
    let description = super::comment::normalize_body(&request.description);
    if !valid_delivery_description(&description) {
        return Err(SourceDeliveryError::Rejected);
    }
    super::comment::require_literal_body(&description)?;
    let issue = request
        .source_issue
        .as_ref()
        .map_or(String::new(), |issue| {
            format!("\n\nCloses #{}", issue.number)
        });
    Ok(format!(
        "{DELIVERY_GENERATED_BODY_START}\n{}{issue}\n{DELIVERY_GENERATED_BODY_END}",
        description
    ))
}

fn refresh_description(existing: &str, replacement: &str) -> Result<String, SourceDeliveryError> {
    let existing = super::comment::normalize_body(existing);
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
    let body = super::comment::normalize_body(&body);
    super::comment::require_literal_body(&body)?;
    Ok(body)
}

pub(super) async fn open_or_update(
    authority: &GitLabDeliveryAuthority,
    request: &SourceReviewRequest,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewReceipt, SourceDeliveryError> {
    let project = authority
        .project(&request.target.repository, credential)
        .await?;
    let description = generated_description(request)?;
    let found = find(authority, &request.target, &request.head_branch, credential).await?;
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
    if authority
        .branch_revision(&request.target.repository, &request.head_branch, credential)
        .await?
        .as_deref()
        != Some(&request.head_revision)
    {
        return Err(SourceDeliveryError::identity(
            "GitLab review creation head is not the published candidate",
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
    let receipt = value.receipt(request, project_id)?;
    require_metadata(&value, &request.title, description)?;
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

async fn reconcile_creation(
    authority: &GitLabDeliveryAuthority,
    request: &SourceReviewRequest,
    original: SourceDeliveryError,
    credential: SourceDeliveryCredential<'_>,
) -> Result<MergeRequest, SourceDeliveryError> {
    match find(authority, &request.target, &request.head_branch, credential).await? {
        Some(value) => Ok(value),
        None if matches!(original.api_status(), Some(400..=499)) => {
            let key = effect_key(&request.target.repository, &request.head_branch);
            authority
                .effects
                .lock()
                .map_err(|_| SourceDeliveryError::Unavailable)?
                .creates
                .remove(&key);
            Err(original)
        }
        None => Err(SourceDeliveryError::api(
            None,
            "GitLab review creation response was lost and no exact review was observed",
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
    let receipt = found.receipt(request, project.id)?;
    if found.state != "opened" {
        return Err(SourceDeliveryError::identity(
            "GitLab run review is no longer open",
        ));
    }
    let body = refresh_description(found.description.as_deref().unwrap_or(""), description)?;
    if found.title.as_deref() == Some(&request.title) && found.description.as_deref() == Some(&body)
    {
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
                json!({"title":request.title, "description":body}),
            ),
            credential,
        )
        .await?;
    value.require_review(&receipt, project.id)?;
    require_metadata(&value, &request.title, &body)?;
    Ok(receipt)
}

fn require_metadata(
    value: &MergeRequest,
    title: &str,
    description: &str,
) -> Result<(), SourceDeliveryError> {
    if value.title.as_deref() != Some(title) || value.description.as_deref() != Some(description) {
        return Err(SourceDeliveryError::identity(
            "GitLab did not confirm the generated review metadata",
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
    observation(&value, receipt, &project)
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
    let Some(pipeline) = &value.head_pipeline else {
        return Ok(
            if project.only_allow_merge_if_pipeline_succeeds == Some(false) {
                SourceChecks::NotRequired
            } else {
                SourceChecks::Pending
            },
        );
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
    settle_merge(authority, (receipt, &project, &key), result)
}

fn settle_merge(
    authority: &GitLabDeliveryAuthority,
    context: (&SourceReviewReceipt, &Project, &str),
    result: Result<MergeRequest, SourceDeliveryError>,
) -> Result<SourceMergeRequestOutcome, SourceDeliveryError> {
    let (receipt, project, key) = context;
    match result {
        Ok(value) => {
            value.require_review(receipt, project.id)?;
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
            "GitLab merge settlement remains uncertain; no repair handoff is authorized",
        ));
    }
    Ok(SourceMergeWithdrawal::Withdrawn)
}

pub(super) async fn feedback(
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
    for discussion in discussions {
        for note in discussion.notes {
            super::comment::require_review_note_scope(&note, &review, project.id)?;
            if !note.system && !note.body.trim().is_empty() {
                items.push(feedback_item(&discussion.id, note));
            }
        }
    }
    authority.merge_request(receipt, credential).await?;
    items.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(SourceReviewFeedback { items })
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
    let target = DeliveryTarget::new(
        &request.review.repository,
        &request.review.target_branch,
        &request.review.head_revision,
    )
    .map_err(|_| SourceDeliveryError::Rejected)?;
    let integrated = transport::reconcile_target(
        authority.git()?,
        SourceTargetReconciliation {
            workspace: &request.workspace,
            target: &target,
            commit_message: "Integrate the captured GitLab target before another review",
        },
        target_revision,
        credential,
    )
    .await?;
    match integrated.outcome {
        SourceReconciliationOutcome::NeedsWork(diagnostic) => {
            Err(SourceDeliveryError::repairable(diagnostic))
        }
        SourceReconciliationOutcome::Unchanged | SourceReconciliationOutcome::Adopted => {
            Err(SourceDeliveryError::identity(
                "GitLab requires rebasing but the candidate already contains the target; the configured merge method requires manual resolution",
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
) -> Result<(), SourceDeliveryError> {
    if known.repository != request.target.repository
        || known.target_branch != request.target.target_branch
        || known.head_branch != request.head_branch
    {
        return Err(SourceDeliveryError::identity(
            "GitLab known review differs from the admitted delivery target",
        ));
    }
    Ok(())
}
