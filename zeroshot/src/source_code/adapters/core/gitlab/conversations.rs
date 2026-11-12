//! Exact-discussion response and resolution effects.
use super::*;
use super::super::review_responses;
use reqwest::Method;
use serde_json::json;
use sha2::{Digest, Sha256};

type ResolutionIdentity = (String, String, String, String, String);

#[derive(Clone, Default)]
pub(super) struct PendingResolutions(
    Arc<tokio::sync::Mutex<BTreeMap<ResolutionIdentity, PendingResolution>>>,
);

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingResolution {
    // Restored host state is already durable. A newly posted reply cannot authorize
    // read-side resolution until this process has received the host's acknowledgement.
    #[serde(skip)]
    checkpointed: bool,
    version: String,
    response_hash: String,
    reply: Option<u64>,
    body_hash: String,
    author: u64,
    confirmed: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    gitlab: Vec<(ResolutionIdentity, PendingResolution)>,
    replies: Vec<super::super::review_responses::ReplyReceipt>,
}

pub(super) async fn checkpoint(
    authority: &GitLabDeliveryAuthority,
) -> Result<Option<serde_json::Value>, SourceDeliveryError> {
    let pending = authority.resolutions.0.lock().await;
    receipt_checkpoint(authority, &pending).map(Some)
}

fn receipt_checkpoint(
    authority: &GitLabDeliveryAuthority,
    pending: &BTreeMap<ResolutionIdentity, PendingResolution>,
) -> Result<serde_json::Value, SourceDeliveryError> {
    let receipts = Checkpoint {
        gitlab: pending
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        replies: authority.published_replies.checkpoint()?,
    };
    review_responses::checkpoint(&receipts)
}

pub(super) async fn restore(
    authority: &GitLabDeliveryAuthority,
    state: serde_json::Value,
) -> Result<(), SourceDeliveryError> {
    review_responses::checkpoint(&state)?;
    let receipts: Checkpoint =
        serde_json::from_value(state).map_err(|_| SourceDeliveryError::Rejected)?;
    let count = receipts.gitlab.len();
    let mut restored: BTreeMap<_, _> = receipts.gitlab.into_iter().collect();
    if count > 100_000 || restored.len() != count {
        return Err(SourceDeliveryError::Rejected);
    }
    for receipt in restored.values_mut() {
        receipt.checkpointed = true;
    }
    let mut pending = authority.resolutions.0.lock().await;
    authority.published_replies.restore(receipts.replies)?;
    *pending = restored;
    Ok(())
}

async fn acknowledge(
    authority: &GitLabDeliveryAuthority,
    pending: &mut BTreeMap<ResolutionIdentity, PendingResolution>,
    checkpoint: &dyn crate::source_code_provider::SourceReviewCheckpoint,
) -> Result<(), SourceDeliveryError> {
    checkpoint
        .retain(receipt_checkpoint(authority, pending)?)
        .await?;
    for receipt in pending.values_mut() {
        receipt.checkpointed = true;
    }
    Ok(())
}

/// Keep the installed receipt (including the last entry) until its removal is durable.
async fn retire(
    authority: &GitLabDeliveryAuthority,
    state: (
        &mut BTreeMap<ResolutionIdentity, PendingResolution>,
        &ResolutionIdentity,
    ),
    checkpoint: &dyn crate::source_code_provider::SourceReviewCheckpoint,
) -> Result<(), SourceDeliveryError> {
    let (pending, key) = state;
    let mut proposed = pending.clone();
    proposed.remove(key);
    acknowledge(authority, &mut proposed, checkpoint).await?;
    *pending = proposed;
    Ok(())
}

/// Settlement may refine replies or remove receipts even when its final forge read fails.
async fn settle_and_acknowledge(
    authority: &GitLabDeliveryAuthority,
    state: (
        &SourceReviewReceipt,
        &ResolutionIdentity,
        &mut BTreeMap<ResolutionIdentity, PendingResolution>,
    ),
    credential: SourceDeliveryCredential<'_>,
    checkpoint: &dyn crate::source_code_provider::SourceReviewCheckpoint,
) -> Result<SourceReviewResolution, SourceDeliveryError> {
    let (review, key, pending) = state;
    let before = receipt_checkpoint(authority, pending)?;
    let mut proposed = pending.clone();
    let mut staged = authority.clone();
    staged.published_replies = authority.published_replies.fork()?;
    let result = settle(
        &staged,
        (review, &key.4, proposed.get_mut(key).unwrap()),
        credential,
    )
    .await;
    if let Ok(result) = result {
        settled(&mut proposed, key, result);
    }
    if receipt_checkpoint(&staged, &proposed)? != before {
        acknowledge(&staged, &mut proposed, checkpoint).await?;
        authority
            .published_replies
            .restore(staged.published_replies.checkpoint()?)?;
        *pending = proposed;
    }
    result
}

fn settled(
    pending: &mut BTreeMap<ResolutionIdentity, PendingResolution>,
    key: &ResolutionIdentity,
    result: SourceReviewResolution,
) {
    if result == SourceReviewResolution::Changed {
        pending.remove(key);
    } else if let Some(attempt) = pending.get_mut(key) {
        attempt.confirmed = true;
    }
}

fn identity(review: &SourceReviewReceipt, thread: &str) -> ResolutionIdentity {
    (
        review.repository.clone(),
        review.review_id.clone(),
        review.target_branch.clone(),
        review.head_branch.clone(),
        thread.to_owned(),
    )
}

/// Earlier verified revisions remain authoritative even when the latest response array is empty.
pub(super) async fn reconcile_resolutions(
    authority: &GitLabDeliveryAuthority,
    receipt: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
    checkpoint: &dyn crate::source_code_provider::SourceReviewCheckpoint,
) -> Result<bool, SourceDeliveryError> {
    let mut pending = authority.resolutions.0.lock().await;
    let keys: Vec<_> = pending
        .keys()
        .filter(|key| **key == identity(receipt, &key.4))
        .cloned()
        .collect();
    if keys.iter().any(|key| !pending[key].checkpointed) {
        return Err(SourceDeliveryError::api(
            None,
            "Pending conversation receipt has not been durably acknowledged",
        ));
    }
    let mut changed = false;
    for key in keys {
        let result = settle_and_acknowledge(
            authority,
            (receipt, &key, &mut pending),
            credential,
            checkpoint,
        )
        .await?;
        changed |= result == SourceReviewResolution::Changed;
    }
    Ok(changed)
}

pub(super) fn version(
    notes: &[wire::Note],
    ignored: Option<u64>,
) -> Result<String, SourceDeliveryError> {
    let mut notes: Vec<_> = notes
        .iter()
        .filter(|note| !note.system && Some(note.id) != ignored)
        .cloned()
        .collect();
    notes.sort_by_key(|note| note.id);
    // Resolution toggles are effects, not substantive changes to the reviewed conversation.
    for note in &mut notes {
        note.resolved = None;
        note.updated_at.clear();
    }
    let bytes = serde_json::to_vec(&notes).map_err(|_| SourceDeliveryError::Rejected)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

async fn current_discussion(
    authority: &GitLabDeliveryAuthority,
    request: (&SourceReviewReceipt, &str),
    credential: SourceDeliveryCredential<'_>,
) -> Result<wire::Discussion, SourceDeliveryError> {
    let (receipt, id) = request;
    let review = authority.merge_request(receipt, credential).await?;
    let project = authority.project(&receipt.repository, credential).await?;
    let value: wire::Discussion = authority
        .api
        .json(
            ApiRequest::get(&[
                "projects",
                &receipt.repository,
                "merge_requests",
                &receipt.review_id,
                "discussions",
                id,
            ]),
            credential,
        )
        .await?;
    if value.id != id {
        return Err(SourceDeliveryError::identity(
            "GitLab discussion identity changed",
        ));
    }
    for note in &value.notes {
        super::comment::require_review_note_scope(note, &review, project.id)?;
    }
    Ok(value)
}

async fn recheck_unmatched_response(
    authority: &GitLabDeliveryAuthority,
    input: (&SourceReviewReceipt, &SourceReviewResponse),
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewResolution, SourceDeliveryError> {
    let (receipt, response) = input;
    let current =
        match current_discussion(authority, (receipt, &response.thread_id), credential).await {
            Ok(current) => current,
            Err(error) if error.api_status() == Some(404) => {
                return Ok(SourceReviewResolution::Changed);
            }
            Err(error) => return Err(error),
        };
    if !unresolved(&current) {
        if version(&current.notes, None)? == response.version {
            return Ok(SourceReviewResolution::Resolved);
        }
        return reopen(
            authority,
            (receipt, &response.thread_id, &current),
            credential,
        )
        .await;
    }
    Ok(SourceReviewResolution::Changed)
}

pub(super) async fn resolve(
    authority: &GitLabDeliveryAuthority,
    request: crate::source_code_provider::SourceReviewResolutionRequest<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewResolution, SourceDeliveryError> {
    let crate::source_code_provider::SourceReviewResolutionRequest {
        review: receipt,
        response,
        checkpoint,
    } = request;
    let mut pending = authority.resolutions.0.lock().await;
    let key = identity(receipt, &response.thread_id);
    let response_hash = review_responses::response_hash(response)?;
    if let Some(attempt) = pending.get(&key) {
        if attempt.response_hash == response_hash || !attempt.confirmed {
            acknowledge(authority, &mut pending, checkpoint).await?;
            let result = settle_and_acknowledge(
                authority,
                (receipt, &key, &mut pending),
                credential,
                checkpoint,
            )
            .await?;
            if result == SourceReviewResolution::Changed
                || pending[&key].response_hash == response_hash
            {
                return Ok(result);
            }
        }
    }
    let feedback = super::review::feedback_snapshot(authority, receipt, credential).await?;
    if !feedback.conversations.iter().any(|thread| {
        thread.id == response.thread_id && thread.version == response.version && thread.authorized
    }) {
        return recheck_unmatched_response(authority, (receipt, response), credential).await;
    }
    let segments = [
        "projects",
        &receipt.repository,
        "merge_requests",
        &receipt.review_id,
        "discussions",
        &response.thread_id,
    ];
    let mut post_segments = segments.to_vec();
    post_segments.push("notes");
    let body = super::comment::literal_body(&review_responses::reply_body(response));
    let author = crate::source_code_provider::SourceCommentAuthority::actor(authority, credential)
        .await?
        .parse::<u64>()
        .map_err(|_| SourceDeliveryError::Rejected)?;
    let mut reserved = pending.clone();
    reserved.insert(
        key.clone(),
        PendingResolution {
            checkpointed: false,
            version: response.version.clone(),
            response_hash: response_hash.clone(),
            reply: Some(u64::MAX),
            body_hash: review_responses::hash(&body),
            author,
            confirmed: false,
        },
    );
    review_responses::checkpoint(&Checkpoint {
        gitlab: reserved
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        replies: authority.published_replies.with_reserved_reply(
            receipt,
            &response.thread_id,
            &body,
        )?,
    })?;
    let mut intent = reserved.remove(&key).unwrap();
    intent.reply = None;
    pending.insert(key.clone(), intent);
    acknowledge(authority, &mut pending, checkpoint).await?;
    let posted = authority
        .api
        .json::<wire::Note>(
            ApiRequest::write(&post_segments, Method::POST, json!({"body":body})),
            credential,
        )
        .await;
    let reply = match posted {
        Ok(reply) => reply,
        Err(error) => {
            if error.api_status() == Some(401) {
                // A completed authentication refusal proves no reply was published.
                // Retire its intent durably before allowing the normal credential retry.
                retire(authority, (&mut pending, &key), checkpoint).await?;
            }
            return Err(error);
        }
    };
    if reply.id == 0 || reply.body != body || reply.system || reply.author.id != author {
        return Err(SourceDeliveryError::identity(
            "GitLab returned invalid response identity",
        ));
    }
    authority
        .published_replies
        .record(receipt, &response.thread_id, (reply.id, &body))?;
    let intent = pending.get_mut(&key).unwrap();
    intent.reply = Some(reply.id);
    intent.checkpointed = false;
    acknowledge(authority, &mut pending, checkpoint).await?;
    settle_and_acknowledge(
        authority,
        (receipt, &key, &mut pending),
        credential,
        checkpoint,
    )
    .await
}

async fn settle(
    authority: &GitLabDeliveryAuthority,
    input: (&SourceReviewReceipt, &str, &mut PendingResolution),
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewResolution, SourceDeliveryError> {
    let (receipt, id, pending) = input;
    let current = current_discussion(authority, (receipt, id), credential).await?;
    if pending.reply.is_none() {
        let reply = review_responses::unique_reply(current.notes.iter().filter_map(|note| {
            (!note.system
                && note.author.id == pending.author
                && review_responses::hash(&note.body) == pending.body_hash)
                .then_some(note.id)
        }))?;
        let note = current.notes.iter().find(|note| note.id == reply).unwrap();
        authority
            .published_replies
            .record(receipt, id, (reply, &note.body))?;
        // The durable intent binds the actor and unique body before the POST, so this
        // ID refinement can be reconstructed without granting any new authority.
        pending.reply = Some(reply);
    }
    if !unchanged(&current, pending)? {
        return reopen(authority, (receipt, id, &current), credential).await;
    }
    if !unresolved(&current) {
        return Ok(SourceReviewResolution::Resolved);
    }
    // A retried write still requires fresh membership for the original participants.
    let mut permissions = BTreeMap::new();
    for note in current.notes.iter().filter(|note| {
        !note.system && Some(note.id) != pending.reply && !note.body.trim().is_empty()
    }) {
        let allowed = match permissions.get(&note.author.id) {
            Some(allowed) => *allowed,
            None => {
                let allowed =
                    super::review::can_repair(authority, receipt, note.author.id, credential)
                        .await?;
                permissions.insert(note.author.id, allowed);
                allowed
            }
        };
        if !allowed {
            return Ok(SourceReviewResolution::Changed);
        }
    }
    set_resolved(authority, (receipt, id, true), credential).await?;
    let after = current_discussion(authority, (receipt, id), credential).await?;
    if !unchanged(&after, pending)? {
        return reopen(authority, (receipt, id, &after), credential).await;
    }
    Ok(if unresolved(&after) {
        SourceReviewResolution::Changed
    } else {
        SourceReviewResolution::Resolved
    })
}

fn unchanged(
    current: &wire::Discussion,
    pending: &PendingResolution,
) -> Result<bool, SourceDeliveryError> {
    Ok(version(&current.notes, pending.reply)? == pending.version
        && current.notes.iter().any(|note| {
            Some(note.id) == pending.reply
                && review_responses::hash(&note.body) == pending.body_hash
                && note.author.id == pending.author
        }))
}

fn unresolved(discussion: &wire::Discussion) -> bool {
    discussion
        .notes
        .iter()
        .any(|note| note.resolvable && note.resolved != Some(true))
}

async fn set_resolved(
    authority: &GitLabDeliveryAuthority,
    input: (&SourceReviewReceipt, &str, bool),
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    let (receipt, id, resolved) = input;
    let result: wire::Discussion = authority
        .api
        .json(
            ApiRequest::write(
                &[
                    "projects",
                    &receipt.repository,
                    "merge_requests",
                    &receipt.review_id,
                    "discussions",
                    id,
                ],
                Method::PUT,
                json!({"resolved":resolved}),
            ),
            credential,
        )
        .await?;
    if result.id != id || unresolved(&result) == resolved {
        return Err(SourceDeliveryError::identity(
            "GitLab did not confirm discussion resolution state",
        ));
    }
    Ok(())
}

async fn reopen(
    authority: &GitLabDeliveryAuthority,
    input: (&SourceReviewReceipt, &str, &wire::Discussion),
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewResolution, SourceDeliveryError> {
    let (receipt, id, current) = input;
    if !unresolved(current) {
        set_resolved(authority, (receipt, id, false), credential).await?;
        if !unresolved(&current_discussion(authority, (receipt, id), credential).await?) {
            return Err(SourceDeliveryError::Unavailable
                .with_context("GitLab did not confirm reopening the changed discussion"));
        }
    }
    Ok(SourceReviewResolution::Changed)
}
