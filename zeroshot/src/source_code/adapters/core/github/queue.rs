//! Exact-head merge submissions and confirmed withdrawal of autonomous queue effects.
use super::*;
use merge_policy::MergeMethod;
use serde_json::{Value, json};

type MergeIdentity = (String, String, String, String, String);
type Submission = Result<MergeReceipt, api::MutationFailure>;

#[derive(Clone, Debug)]
enum MergeReceipt {
    Direct,
    Queued(QueueAdmission),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct QueueAdmission {
    entry_id: String,
    enqueued_at: time::OffsetDateTime,
    pull_request_id: String,
}

mod outcome;
mod recovery;
pub(super) use outcome::observe_removal;
pub(super) use recovery::{RecoveryPolicy, recovery_policy};

#[derive(Clone, Debug, Default)]
pub(super) struct MergeEffects(Arc<tokio::sync::Mutex<BTreeMap<MergeIdentity, MergeEffect>>>);

#[derive(Debug)]
struct MergeEffect {
    task: Option<tokio::task::JoinHandle<Submission>>,
    settled: Option<Submission>,
    method: MergeMethod,
    queue_uncertain: bool,
    direct_confirmation_required: bool,
    recovered_removals: std::collections::BTreeSet<String>,
    recoverable: Option<String>,
}

impl MergeEffect {
    fn admission(&self) -> Option<&QueueAdmission> {
        match self.settled.as_ref()? {
            Ok(MergeReceipt::Queued(admission)) => Some(admission),
            _ => None,
        }
    }

    fn retains_submission(&self, method: MergeMethod) -> bool {
        // Acceptance awaits observation. An unrepeatable indeterminate response keeps
        // ownership even when later reads show an empty queue.
        match self.settled.as_ref() {
            Some(Ok(_)) => self.recoverable.is_none(),
            Some(Err(failure)) => self.retains_failure(failure, method),
            None => self.task.is_some(),
        }
    }

    fn retains_failure(&self, failure: &api::MutationFailure, method: MergeMethod) -> bool {
        if failure.error.cleanup_unconfirmed()
            || self.method != method && self.has_unresolved_effect()
        {
            return true;
        }
        match failure.rejection {
            Some(api::MutationRejection::Policy) => self.method == method,
            Some(_) => false,
            None => failure.error.api_status().is_none() && !failure.error.retryable_operation(),
        }
    }

    fn has_unresolved_effect(&self) -> bool {
        self.queue_uncertain || self.direct_confirmation_required || self.task.is_some()
    }

    async fn settle(&mut self) -> Submission {
        if let Some(task) = self.task.as_mut() {
            let result = task.await.unwrap_or_else(|_| {
                Err(api::MutationFailure::unknown(SourceDeliveryError::api(
                    None,
                    "GitHub merge submission task failed before confirming the remote effect",
                )))
            });
            let uncertain = matches!(&result, Err(failure)
                if failure.rejection.is_none() && uncertain_submission(&failure.error));
            if self.method == MergeMethod::Queue {
                self.queue_uncertain |= uncertain;
            } else {
                self.direct_confirmation_required |= uncertain || result.is_ok();
            }
            self.settled = Some(result);
            self.task = None;
        }
        self.settled
            .clone()
            .unwrap_or_else(|| Err(api::MutationFailure::unknown(invalid_receipt())))
    }
}

const ENQUEUE: &str = r#"
mutation($pullRequestId: ID!, $expectedHeadOid: GitObjectID!) {
  enqueuePullRequest(input: {pullRequestId: $pullRequestId, expectedHeadOid: $expectedHeadOid}) {
    mergeQueueEntry {
      id enqueuedAt
      pullRequest { id number repository { nameWithOwner } baseRefName headRefName headRefOid }
    }
  }
}
"#;
const MERGE: &str = r#"
mutation($pullRequestId: ID!, $expectedHeadOid: GitObjectID!, $method: PullRequestMergeMethod!) {
  mergePullRequest(input: {
    pullRequestId: $pullRequestId, expectedHeadOid: $expectedHeadOid, mergeMethod: $method
  }) {
    pullRequest {
      id number repository { nameWithOwner } baseRefName headRefName headRefOid
      merged mergeCommit { oid }
    }
  }
}
"#;
const DEQUEUE: &str = r#"
mutation($pullRequestId: ID!) {
  dequeuePullRequest(input: {id: $pullRequestId}) { clientMutationId }
}
"#;
const DISABLE_AUTO_MERGE: &str = r#"
mutation($pullRequestId: ID!) {
  disablePullRequestAutoMerge(input: {pullRequestId: $pullRequestId}) { clientMutationId }
}
"#;

pub(super) async fn merge(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    (method, node_id): (MergeMethod, &str),
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), api::MutationFailure> {
    if node_id.trim().is_empty() {
        return Err(api::MutationFailure::unknown(invalid_receipt()));
    }
    let mut effects = authority.merge_effects.0.lock().await;
    let effect = effects
        .entry(identity(review))
        .or_insert_with(|| MergeEffect {
            task: None,
            settled: None,
            method,
            queue_uncertain: false,
            direct_confirmation_required: false,
            recovered_removals: Default::default(),
            recoverable: None,
        });
    if !effect.retains_submission(method) {
        if let Some(removal) = effect.recoverable.take() {
            effect.recovered_removals.insert(removal);
        }
        let authority = authority.clone();
        let token = credential.expose().to_owned();
        let review = review.clone();
        let node_id = node_id.to_owned();
        effect.method = method;
        effect.settled = None;
        effect.task = Some(tokio::spawn(async move {
            submit(
                &authority,
                &review,
                (method, &node_id),
                SourceDeliveryCredential::token(&token),
            )
            .await
        }));
    }
    // Retain the task when this future is cancelled; withdrawal drains that same request first.
    effect.settle().await.map(|_| ())
}

async fn submit(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    (method, node_id): (MergeMethod, &str),
    credential: SourceDeliveryCredential<'_>,
) -> Submission {
    let mut variables = json!({
        "pullRequestId": node_id,
        "expectedHeadOid": review.head_revision,
    });
    let direct = match method {
        MergeMethod::Queue => None,
        MergeMethod::Merge => Some("MERGE"),
        MergeMethod::Squash => Some("SQUASH"),
        MergeMethod::Rebase => Some("REBASE"),
    };
    let Some(direct) = direct else {
        let value = authority
            .mutation((ENQUEUE, "enqueuePullRequest"), &variables, credential)
            .await?;
        return confirm_receipt(value, review, node_id)
            .map(MergeReceipt::Queued)
            .map_err(api::MutationFailure::unknown);
    };
    variables["method"] = json!(direct);
    let value = authority
        .mutation((MERGE, "mergePullRequest"), &variables, credential)
        .await?;
    let pull = &value["data"]["mergePullRequest"]["pullRequest"];
    (matches_review(pull, review, node_id)
        && pull["merged"] == true
        && pull
            .pointer("/mergeCommit/oid")
            .and_then(Value::as_str)
            .is_some_and(valid_revision))
    .then_some(MergeReceipt::Direct)
    .ok_or_else(|| {
        api::MutationFailure::unknown(SourceDeliveryError::api(
            None,
            "GitHub returned an invalid direct merge receipt or identity",
        ))
    })
}

fn confirm_receipt(
    value: Value,
    review: &SourceReviewReceipt,
    node_id: &str,
) -> Result<QueueAdmission, SourceDeliveryError> {
    let entry = value
        .pointer("/data/enqueuePullRequest/mergeQueueEntry")
        .ok_or_else(invalid_receipt)?;
    let pull = entry.get("pullRequest").ok_or_else(invalid_receipt)?;
    let valid = entry
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
        && matches_review(pull, review, node_id);
    valid
        .then_some(QueueAdmission {
            entry_id: entry["id"].as_str().ok_or_else(invalid_receipt)?.to_owned(),
            enqueued_at: queue_timestamp(&entry["enqueuedAt"]).ok_or_else(invalid_receipt)?,
            pull_request_id: node_id.to_owned(),
        })
        .ok_or_else(invalid_receipt)
}

fn matches_review(pull: &Value, review: &SourceReviewReceipt, node_id: &str) -> bool {
    pull.get("id").and_then(Value::as_str) == Some(node_id)
        && pull
            .get("number")
            .and_then(Value::as_u64)
            .map(|number| number.to_string())
            .as_deref()
            == Some(review.review_id.as_str())
        && pull
            .pointer("/repository/nameWithOwner")
            .and_then(Value::as_str)
            == Some(review.repository.as_str())
        && pull.get("baseRefName").and_then(Value::as_str) == Some(review.target_branch.as_str())
        && pull.get("headRefName").and_then(Value::as_str) == Some(review.head_branch.as_str())
        && pull.get("headRefOid").and_then(Value::as_str) == Some(review.head_revision.as_str())
}

fn queue_timestamp(value: &Value) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(
        value.as_str()?,
        &time::format_description::well_known::Rfc3339,
    )
    .ok()
}

pub(super) async fn withdraw(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceMergeWithdrawal, SourceDeliveryError> {
    let key = identity(review);
    let mut effects = authority.merge_effects.0.lock().await;
    let (queue_uncertain, direct_confirmation_required) = match effects.get_mut(&key) {
        Some(effect) => match effect.settle().await {
            Err(failure) if failure.error.cleanup_unconfirmed() => return Err(failure.error),
            _ => (effect.queue_uncertain, effect.direct_confirmation_required),
        },
        None => (false, false),
    };
    let result = withdraw_confirmed(authority, review, (credential, queue_uncertain)).await;
    if direct_confirmation_required && matches!(result, Ok(SourceMergeWithdrawal::Withdrawn)) {
        return Err(SourceDeliveryError::api(
            None,
            "direct merge submission remains unconfirmed; an empty observation cannot authorize repair",
        ));
    }
    if result.is_ok() {
        effects.remove(&key);
    }
    result
}

fn uncertain_submission(error: &SourceDeliveryError) -> bool {
    error.cleanup_unconfirmed() || error.api_status().is_none() || error.retryable_operation()
}

async fn withdraw_confirmed(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    context: (SourceDeliveryCredential<'_>, bool),
) -> Result<SourceMergeWithdrawal, SourceDeliveryError> {
    let (credential, queue_uncertain) = context;
    let mut snapshot = authority.policy_snapshot(review, credential).await?;
    require_observed_submission(&snapshot, queue_uncertain)?;
    // Disable old CLI auto-merge intent first: otherwise removing an entry could re-enqueue it.
    for query in [DISABLE_AUTO_MERGE, DEQUEUE] {
        if let Some(settled) = withdrawal(&snapshot)? {
            return Ok(settled);
        }
        let needed = if query == DISABLE_AUTO_MERGE {
            snapshot.auto_merge_enabled
        } else {
            snapshot.is_in_merge_queue
        };
        if !needed {
            continue;
        }
        snapshot = withdraw_step(
            authority,
            (review, &snapshot.pull_request_id),
            query,
            credential,
        )
        .await?;
    }
    withdrawal(&snapshot)?.ok_or_else(|| {
        SourceDeliveryError::api(
            None,
            "GitHub did not confirm withdrawal of the queued or auto-merge request",
        )
    })
}

fn require_observed_submission(
    snapshot: &PolicySnapshot,
    queue_uncertain: bool,
) -> Result<(), SourceDeliveryError> {
    if queue_uncertain
        && !snapshot.is_in_merge_queue
        && !matches!(snapshot.state, SourceReviewState::Merged { .. })
    {
        return Err(SourceDeliveryError::api(
            None,
            "merge queue submission remains unconfirmed; an empty observation cannot authorize repair",
        ));
    }
    Ok(())
}

async fn withdraw_step(
    authority: &GhCliDeliveryAuthority,
    identity: (&SourceReviewReceipt, &str),
    query: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Result<PolicySnapshot, SourceDeliveryError> {
    let (review, node_id) = identity;
    let field = if query == DISABLE_AUTO_MERGE {
        "disablePullRequestAutoMerge"
    } else {
        "dequeuePullRequest"
    };
    let submitted = authority
        .mutation(
            (query, field),
            &json!({"pullRequestId": node_id}),
            credential,
        )
        .await
        .map_err(|failure| failure.error);
    let observed = authority.policy_snapshot(review, credential).await;
    if let Ok(snapshot) = &observed {
        if withdrawal(snapshot)?.is_some()
            || query == DISABLE_AUTO_MERGE && !snapshot.auto_merge_enabled
        {
            return observed;
        }
    }
    submitted?;
    observed
}

fn withdrawal(
    snapshot: &PolicySnapshot,
) -> Result<Option<SourceMergeWithdrawal>, SourceDeliveryError> {
    if snapshot.pull_request_id.is_empty() {
        return Err(invalid_receipt());
    }
    if let SourceReviewState::Merged { merge_revision } = &snapshot.state {
        return Ok(Some(SourceMergeWithdrawal::Merged {
            merge_revision: merge_revision.clone(),
        }));
    }
    Ok(
        (!snapshot.is_in_merge_queue && !snapshot.auto_merge_enabled)
            .then_some(SourceMergeWithdrawal::Withdrawn),
    )
}

fn identity(review: &SourceReviewReceipt) -> MergeIdentity {
    (
        review.repository.clone(),
        review.review_id.clone(),
        review.target_branch.clone(),
        review.head_branch.clone(),
        review.head_revision.clone(),
    )
}

fn invalid_receipt() -> SourceDeliveryError {
    SourceDeliveryError::api(
        None,
        "GitHub returned an invalid merge queue receipt or identity",
    )
}
