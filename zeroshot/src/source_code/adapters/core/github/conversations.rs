//! Review-thread identity and effects stay behind the trusted GitHub authority.
use std::collections::BTreeSet;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use super::*;
use super::super::review_responses;
use crate::source_code_provider::{
    SourceReviewConversation, SourceReviewResponse, SourceReviewResolution,
};
use super::feedback::ReviewCommentWire;

type ResolutionIdentity = (String, String, String, String, String);

#[derive(Clone, Debug, Default)]
pub(super) struct PendingResolutions(
    Arc<tokio::sync::Mutex<BTreeMap<ResolutionIdentity, PendingResolution>>>,
);

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
    author: String,
    confirmed: bool,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    github: Vec<(ResolutionIdentity, PendingResolution)>,
    replies: Vec<super::super::review_responses::ReplyReceipt>,
}

pub(super) async fn checkpoint(
    authority: &GhCliDeliveryAuthority,
) -> Result<Option<serde_json::Value>, SourceDeliveryError> {
    let pending = authority.resolutions.0.lock().await;
    receipt_checkpoint(authority, &pending).map(Some)
}

fn receipt_checkpoint(
    authority: &GhCliDeliveryAuthority,
    pending: &BTreeMap<ResolutionIdentity, PendingResolution>,
) -> Result<serde_json::Value, SourceDeliveryError> {
    let receipts = Checkpoint {
        github: pending
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        replies: authority.published_replies.checkpoint()?,
    };
    review_responses::checkpoint(&receipts)
}

pub(super) async fn restore(
    authority: &GhCliDeliveryAuthority,
    state: serde_json::Value,
) -> Result<(), SourceDeliveryError> {
    review_responses::checkpoint(&state)?;
    let receipts: Checkpoint =
        serde_json::from_value(state).map_err(|_| SourceDeliveryError::Rejected)?;
    let count = receipts.github.len();
    let mut restored: BTreeMap<_, _> = receipts.github.into_iter().collect();
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
    authority: &GhCliDeliveryAuthority,
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
    authority: &GhCliDeliveryAuthority,
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
    authority: &GhCliDeliveryAuthority,
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

/// Recheck every owned resolution, including confirmed effects from earlier repairs.
pub(super) async fn reconcile_resolutions(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
    checkpoint: &dyn crate::source_code_provider::SourceReviewCheckpoint,
) -> Result<bool, SourceDeliveryError> {
    let mut pending = authority.resolutions.0.lock().await;
    let keys: Vec<_> = pending
        .keys()
        .filter(|key| **key == identity(review, &key.4))
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
            (review, &key, &mut pending),
            credential,
            checkpoint,
        )
        .await?;
        changed |= result == SourceReviewResolution::Changed;
    }
    Ok(changed)
}

const QUERY: &str = r#"
query ReviewConversations($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    nameWithOwner
    pullRequest(number: $number) {
      number baseRefName headRefName headRefOid
      reviewThreads(first: 100, after: $endCursor) {
        totalCount pageInfo { hasNextPage endCursor }
        nodes { id isResolved comments(first: 1) { totalCount nodes { fullDatabaseId } } }
      }
    }
  }
}"#;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Thread {
    id: String,
    is_resolved: bool,
    comments: Roots,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Roots {
    total_count: usize,
    nodes: Vec<Root>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Root {
    full_database_id: String,
}

fn database_id(value: &str) -> Result<u64, SourceDeliveryError> {
    value
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0 && id.to_string() == value)
        .ok_or_else(invalid)
}

pub(super) struct Snapshot {
    pub(super) comment_ids: BTreeSet<u64>,
    versions: BTreeMap<String, String>,
    comment_threads: BTreeMap<u64, String>,
    comment_bodies: BTreeMap<u64, String>,
    comment_authors: BTreeMap<u64, String>,
    pub(super) items: Vec<SourceReviewFeedbackItem>,
    pub(super) conversations: Vec<SourceReviewConversation>,
}

pub(super) async fn read(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    comments: &[ReviewCommentWire],
    credential: SourceDeliveryCredential<'_>,
) -> Result<Snapshot, SourceDeliveryError> {
    snapshot(authority, (review, comments, None), credential).await
}

fn thread_page(
    review: &SourceReviewReceipt,
    page: &serde_json::Value,
    pagination: (usize, usize, &mut Option<u64>),
    credential: SourceDeliveryCredential<'_>,
) -> Result<Vec<Thread>, SourceDeliveryError> {
    let (index, pages, total) = pagination;
    let repository = &page["data"]["repository"];
    let pr = &repository["pullRequest"];
    if repository["nameWithOwner"].as_str() != Some(&review.repository)
        || pr["number"].as_u64().map(|id| id.to_string()).as_deref() != Some(&review.review_id)
        || pr["baseRefName"].as_str() != Some(&review.target_branch)
        || pr["headRefName"].as_str() != Some(&review.head_branch)
        || pr["headRefOid"].as_str() != Some(&review.head_revision)
    {
        return Err(SourceDeliveryError::identity(
            "GitHub conversation review/head identity changed",
        ));
    }
    let connection = &pr["reviewThreads"];
    let count = connection["totalCount"].as_u64().ok_or_else(invalid)?;
    if count > 100_000 || total.is_some_and(|previous| previous != count) {
        return Err(invalid());
    }
    *total = Some(count);
    let has_next = connection["pageInfo"]["hasNextPage"]
        .as_bool()
        .ok_or_else(invalid)?;
    if has_next != (index + 1 < pages)
        || (has_next
            && connection["pageInfo"]["endCursor"]
                .as_str()
                .is_none_or(str::is_empty))
    {
        return Err(invalid());
    }
    super::api::decode_response(connection["nodes"].clone(), credential)
}

fn thread_comments<'a>(
    thread: &Thread,
    comments_by_root: &BTreeMap<u64, Vec<&'a ReviewCommentWire>>,
) -> Result<Vec<&'a ReviewCommentWire>, SourceDeliveryError> {
    let [root] = thread.comments.nodes.as_slice() else {
        return Err(invalid());
    };
    let root = database_id(&root.full_database_id)?;
    let entries = comments_by_root.get(&root).cloned().unwrap_or_default();
    if entries.len() != thread.comments.total_count || !entries.iter().any(|c| c.id == root) {
        return Err(SourceDeliveryError::Unavailable
            .with_context("GitHub conversation comments changed during inspection"));
    }
    Ok(entries)
}

async fn snapshot(
    authority: &GhCliDeliveryAuthority,
    input: (&SourceReviewReceipt, &[ReviewCommentWire], Option<u64>),
    credential: SourceDeliveryCredential<'_>,
) -> Result<Snapshot, SourceDeliveryError> {
    let (review, comments, ignored_reply) = input;
    let pages = authority
        .api(&review_query_arguments(review, QUERY)?, credential)
        .await?;
    let pages = pages.as_array().ok_or_else(invalid)?;
    if pages.is_empty() {
        return Err(invalid());
    }
    let mut result = Snapshot {
        versions: BTreeMap::new(),
        comment_threads: BTreeMap::new(),
        comment_bodies: BTreeMap::new(),
        comment_authors: BTreeMap::new(),
        comment_ids: BTreeSet::new(),
        items: Vec::new(),
        conversations: Vec::new(),
    };
    let mut comments_by_root: BTreeMap<u64, Vec<&ReviewCommentWire>> = BTreeMap::new();
    for comment in comments {
        comments_by_root
            .entry(comment.in_reply_to_id.unwrap_or(comment.id))
            .or_default()
            .push(comment);
    }
    let mut seen = BTreeSet::new();
    let mut permissions = BTreeMap::new();
    let mut total = None;
    for (index, page) in pages.iter().enumerate() {
        let threads = thread_page(review, page, (index, pages.len(), &mut total), credential)?;
        for thread in threads {
            if thread.id.is_empty() || !seen.insert(thread.id.clone()) {
                return Err(invalid());
            }
            let mut entries = thread_comments(&thread, &comments_by_root)?;
            for comment in &entries {
                if result
                    .comment_threads
                    .insert(comment.id, thread.id.clone())
                    .is_some()
                {
                    return Err(invalid());
                }
                result.comment_ids.insert(comment.id);
                result
                    .comment_bodies
                    .insert(comment.id, comment.body.clone());
                if let Some(author) = &comment.user.node_id {
                    result.comment_authors.insert(comment.id, author.clone());
                }
            }
            entries.retain(|comment| Some(comment.id) != ignored_reply);
            entries.sort_by_key(|comment| comment.id);
            let mut digest = Sha256::new();
            let mut authorized_digest = Sha256::new();
            let mut body = String::new();
            let mut authorized = true;
            for comment in entries {
                // Current commit/line positions can move when a repair is pushed. Only
                // authored content, stable identity and original location revise approval.
                let revision = serde_json::to_vec(&serde_json::json!({
                    "id":comment.id,"authorId":comment.user.id,"authorType":comment.user.kind,"body":comment.body,"updatedAt":comment.updated_at,
                    "path":comment.path,"originalLine":comment.original_line,"replyTo":comment.in_reply_to_id,
                })).map_err(|_| invalid())?;
                digest.update(&revision);
                digest.update([0]);
                if comment.body.trim().is_empty() {
                    continue;
                }
                if thread.is_resolved {
                    continue;
                }
                if authority.published_replies.matches(
                    review,
                    &thread.id,
                    (comment.id, &comment.body),
                )? {
                    continue;
                }
                let allowed = match permissions.get(&(comment.user.id, comment.user.kind.clone())) {
                    Some(allowed) => *allowed,
                    None => {
                        let allowed =
                            feedback::can_repair(authority, review, &comment.user, credential)
                                .await?;
                        permissions.insert((comment.user.id, comment.user.kind.clone()), allowed);
                        allowed
                    }
                };
                authorized &= allowed;
                if !allowed {
                    continue;
                }
                authorized_digest.update(&revision);
                authorized_digest.update([0]);
                body.push_str(&format!(
                    "\n{} (author id={}, comment={}):\n{}\n",
                    comment.user.login, comment.user.id, comment.id, comment.body
                ));
            }
            let version = format!("{:x}", digest.finalize());
            result.versions.insert(thread.id.clone(), version.clone());
            if thread.is_resolved {
                continue;
            }
            if !body.trim().is_empty() {
                result.items.push(SourceReviewFeedbackItem {
                    key: format!("conversation:{}", thread.id),
                    version: format!("{:x}", authorized_digest.finalize()),
                    author: "conversation participants".to_owned(),
                    location: Some(thread.id.clone()),
                    body,
                });
            }
            result.conversations.push(SourceReviewConversation {
                id: thread.id,
                version,
                authorized,
            });
        }
    }
    if total != Some(seen.len() as u64) {
        return Err(invalid());
    }
    Ok(result)
}

async fn fresh(
    authority: &GhCliDeliveryAuthority,
    input: (&SourceReviewReceipt, Option<u64>),
    credential: SourceDeliveryCredential<'_>,
) -> Result<Snapshot, SourceDeliveryError> {
    let (review, ignored) = input;
    let pr = authority.pull_request(review, credential).await?;
    require_review_identity(&pr, review)?;
    let comments = feedback::pages::<ReviewCommentWire>(
        authority,
        format!(
            "repos/{}/pulls/{}/comments?per_page=100",
            review.repository, review.review_id
        ),
        credential,
    )
    .await?;
    let snapshot = snapshot(authority, (review, &comments, ignored), credential).await?;
    require_review_identity(&authority.pull_request(review, credential).await?, review)?;
    Ok(snapshot)
}

pub(super) async fn resolve(
    authority: &GhCliDeliveryAuthority,
    request: crate::source_code_provider::SourceReviewResolutionRequest<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewResolution, SourceDeliveryError> {
    let crate::source_code_provider::SourceReviewResolutionRequest {
        review,
        response,
        checkpoint,
    } = request;
    let mut pending = authority.resolutions.0.lock().await;
    let key = identity(review, &response.thread_id);
    let response_hash = review_responses::response_hash(response)?;
    if let Some(attempt) = pending.get(&key) {
        if attempt.response_hash == response_hash || !attempt.confirmed {
            acknowledge(authority, &mut pending, checkpoint).await?;
            let result = settle_and_acknowledge(
                authority,
                (review, &key, &mut pending),
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
    let before = fresh(authority, (review, None), credential).await?;
    if !matches_response(&before, response) {
        if let Some(version) = before.versions.get(&response.thread_id)
            && !before
                .conversations
                .iter()
                .any(|thread| thread.id == response.thread_id)
        {
            if version == &response.version {
                return Ok(SourceReviewResolution::Resolved);
            }
            // The durable reviewed response identifies a possible interrupted effect,
            // but no retained reply receipt proves this changed resolved revision.
            return reopen(
                authority,
                (review, &response.thread_id, &before),
                credential,
            )
            .await;
        }
        return Ok(SourceReviewResolution::Changed);
    }
    let body = review_responses::reply_body(response);
    let author =
        crate::source_code_provider::SourceCommentAuthority::actor(authority, credential).await?;
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
        github: reserved
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        replies: authority.published_replies.with_reserved_reply(
            review,
            &response.thread_id,
            &body,
        )?,
    })?;
    let mut intent = reserved.remove(&key).unwrap();
    intent.reply = None;
    pending.insert(key.clone(), intent);
    acknowledge(authority, &mut pending, checkpoint).await?;
    let posted = authority.api(&[
        "graphql".to_owned(), "-f".to_owned(),
        "query=mutation($thread:ID!,$body:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$thread,body:$body}){comment{fullDatabaseId}}}".to_owned(),
        "-f".to_owned(), format!("thread={}", response.thread_id), "-f".to_owned(), format!("body={body}"),
    ], credential).await;
    let value = match posted {
        Ok(value) => value,
        Err(error) => {
            if error.api_status() == Some(401) {
                // A completed authentication refusal proves no reply was published.
                // Retire its intent durably before allowing the normal credential retry.
                retire(authority, (&mut pending, &key), checkpoint).await?;
            }
            return Err(error);
        }
    };
    let reply = database_id(
        value["data"]["addPullRequestReviewThreadReply"]["comment"]["fullDatabaseId"]
            .as_str()
            .ok_or_else(invalid)?,
    )?;
    authority
        .published_replies
        .record(review, &response.thread_id, (reply, &body))?;
    let intent = pending.get_mut(&key).unwrap();
    intent.reply = Some(reply);
    intent.checkpointed = false;
    acknowledge(authority, &mut pending, checkpoint).await?;
    settle_and_acknowledge(
        authority,
        (review, &key, &mut pending),
        credential,
        checkpoint,
    )
    .await
}

async fn settle(
    authority: &GhCliDeliveryAuthority,
    input: (&SourceReviewReceipt, &str, &mut PendingResolution),
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewResolution, SourceDeliveryError> {
    let (review, id, pending) = input;
    if pending.reply.is_none() {
        let snapshot = fresh(authority, (review, None), credential).await?;
        let reply = review_responses::unique_reply(snapshot.comment_bodies.iter().filter_map(
            |(reply, body)| {
                (snapshot
                    .comment_threads
                    .get(reply)
                    .is_some_and(|thread| thread == id)
                    && snapshot.comment_authors.get(reply) == Some(&pending.author)
                    && review_responses::hash(body) == pending.body_hash)
                    .then_some(*reply)
            },
        ))?;
        authority.published_replies.record(
            review,
            id,
            (reply, &snapshot.comment_bodies[&reply]),
        )?;
        // The acknowledged intent already binds this exact actor, unique body and revision.
        // Refining its forge ID is recoverable from that intent even if this read is interrupted.
        pending.reply = Some(reply);
    }
    let before = fresh(authority, (review, pending.reply), credential).await?;
    if !unchanged(&before, id, pending) {
        return reopen(authority, (review, id, &before), credential).await;
    }
    if !before.conversations.iter().any(|thread| thread.id == id) {
        return Ok(SourceReviewResolution::Resolved);
    }
    if !matches_revision(&before, id, &pending.version) {
        return Ok(SourceReviewResolution::Changed);
    }
    set_resolved(authority, (id, true), credential).await?;
    let after = fresh(authority, (review, pending.reply), credential).await?;
    if !unchanged(&after, id, pending) {
        return reopen(authority, (review, id, &after), credential).await;
    }
    if after.conversations.iter().any(|thread| thread.id == id) {
        return Ok(SourceReviewResolution::Changed);
    }
    Ok(SourceReviewResolution::Resolved)
}

fn unchanged(snapshot: &Snapshot, id: &str, pending: &PendingResolution) -> bool {
    let Some(reply) = pending.reply else {
        return false;
    };
    snapshot.versions.get(id) == Some(&pending.version)
        && snapshot
            .comment_bodies
            .get(&reply)
            .is_some_and(|body| review_responses::hash(body) == pending.body_hash)
        && snapshot
            .comment_threads
            .get(&reply)
            .is_some_and(|thread| thread == id)
        && snapshot.comment_authors.get(&reply) == Some(&pending.author)
}

async fn reopen(
    authority: &GhCliDeliveryAuthority,
    input: (&SourceReviewReceipt, &str, &Snapshot),
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceReviewResolution, SourceDeliveryError> {
    let (review, id, snapshot) = input;
    if !snapshot.conversations.iter().any(|thread| thread.id == id) {
        set_resolved(authority, (id, false), credential).await?;
        let reopened = fresh(authority, (review, None), credential).await?;
        if !reopened.conversations.iter().any(|thread| thread.id == id) {
            return Err(SourceDeliveryError::Unavailable
                .with_context("GitHub did not confirm reopening the changed conversation"));
        }
    }
    Ok(SourceReviewResolution::Changed)
}

fn matches_response(snapshot: &Snapshot, response: &SourceReviewResponse) -> bool {
    matches_revision(snapshot, &response.thread_id, &response.version)
}

fn matches_revision(snapshot: &Snapshot, id: &str, version: &str) -> bool {
    snapshot
        .conversations
        .iter()
        .any(|thread| thread.id == id && thread.version == version && thread.authorized)
}

async fn set_resolved(
    authority: &GhCliDeliveryAuthority,
    input: (&str, bool),
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    let (id, resolved) = input;
    let mutation = if resolved {
        "resolveReviewThread"
    } else {
        "unresolveReviewThread"
    };
    let value = authority.api(&[
        "graphql".to_owned(), "-f".to_owned(),
        format!("query=mutation($thread:ID!){{{mutation}(input:{{threadId:$thread}}){{thread{{id isResolved}}}}}}"),
        "-f".to_owned(), format!("thread={id}"),
    ], credential).await?;
    let thread = &value["data"][mutation]["thread"];
    if thread["id"].as_str() != Some(id) || thread["isResolved"].as_bool() != Some(resolved) {
        return Err(invalid());
    }
    Ok(())
}

fn invalid() -> SourceDeliveryError {
    SourceDeliveryError::api(
        None,
        "GitHub returned incomplete or inconsistent review conversations",
    )
}

pub(super) async fn has_blocker(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<bool, SourceDeliveryError> {
    Ok(!fresh(authority, (review, None), credential)
        .await?
        .conversations
        .is_empty())
}

#[cfg(all(test, unix))]
#[path = "conversations/test_support.rs"]
pub(crate) mod test_support;
#[cfg(all(test, unix))]
#[path = "conversations/tests.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "conversations/recovery_tests.rs"]
mod recovery_tests;
