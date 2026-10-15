//! Exact-head queue admission and confirmed withdrawal of autonomous merge effects.
use super::*;
use serde_json::Value;

type QueueIdentity = (String, String, String, String, String);
type Submission = Result<(), SourceDeliveryError>;

#[derive(Clone, Debug, Default)]
pub(super) struct QueueEffects(Arc<tokio::sync::Mutex<BTreeMap<QueueIdentity, QueueEffect>>>);

#[derive(Debug)]
struct QueueEffect {
    task: Option<tokio::task::JoinHandle<Submission>>,
    settled: Option<Submission>,
    uncertain: bool,
}

impl QueueEffect {
    async fn settle(&mut self) -> Submission {
        if let Some(task) = self.task.as_mut() {
            let result = task.await.unwrap_or_else(|_| Err(submission_panicked()));
            self.uncertain |= matches!(&result, Err(error) if uncertain_submission(error));
            self.settled = Some(result);
            self.task = None;
        }
        self.settled
            .clone()
            .unwrap_or_else(|| Err(invalid_receipt()))
    }
}

const ENQUEUE: &str = r#"
mutation($pullRequestId: ID!, $expectedHeadOid: GitObjectID!) {
  enqueuePullRequest(input: {pullRequestId: $pullRequestId, expectedHeadOid: $expectedHeadOid}) {
    mergeQueueEntry {
      id
      pullRequest { id number repository { nameWithOwner } baseRefName headRefName headRefOid }
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

pub(super) async fn enqueue(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    node_id: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Submission {
    let mut effects = authority.queue_effects.0.lock().await;
    let key = identity(review);
    if let Some(effect) = effects.get_mut(&key) {
        if effect.task.is_some()
            || effect.settled.as_ref().is_some_and(|result| {
                matches!(result, Err(error) if error.cleanup_unconfirmed()
                    || uncertain_submission(error) && !error.retryable_operation())
            })
        {
            return effect.settle().await;
        }
    }
    {
        let uncertain = effects.get(&key).is_some_and(|effect| effect.uncertain);
        let mut command = mutation(authority, ENQUEUE, node_id, credential)?;
        command.args(["-f", &format!("expectedHeadOid={}", review.head_revision)]);
        let token = credential.expose().to_owned();
        let deadline = authority.config.api_deadline;
        let review = review.clone();
        let node_id = node_id.to_owned();
        let task = tokio::spawn(async move {
            let value =
                api::command_json(command, deadline, SourceDeliveryCredential::token(&token))
                    .await?;
            confirm_receipt(value, &review, &node_id)
        });
        effects.insert(
            key.clone(),
            QueueEffect {
                task: Some(task),
                settled: None,
                uncertain,
            },
        );
    }
    // Retain the task when this future is cancelled; withdrawal drains that same request first.
    effects
        .get_mut(&key)
        .ok_or_else(invalid_receipt)?
        .settle()
        .await
}

fn confirm_receipt(value: Value, review: &SourceReviewReceipt, node_id: &str) -> Submission {
    let entry = value
        .pointer("/data/enqueuePullRequest/mergeQueueEntry")
        .ok_or_else(invalid_receipt)?;
    let pull = entry.get("pullRequest").ok_or_else(invalid_receipt)?;
    let valid = entry
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|id| !id.is_empty())
        && pull.get("id").and_then(Value::as_str) == Some(node_id)
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
        && pull.get("headRefOid").and_then(Value::as_str) == Some(review.head_revision.as_str());
    valid.then_some(()).ok_or_else(invalid_receipt)
}

pub(super) async fn withdraw(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    credential: SourceDeliveryCredential<'_>,
) -> Result<SourceMergeWithdrawal, SourceDeliveryError> {
    let key = identity(review);
    let mut effects = authority.queue_effects.0.lock().await;
    let uncertain = match effects.get_mut(&key) {
        Some(effect) => match effect.settle().await {
            Err(error) if error.cleanup_unconfirmed() => return Err(error),
            _ => effect.uncertain,
        },
        None => false,
    };
    let result = withdraw_confirmed(authority, review, (credential, uncertain)).await;
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
    let (credential, uncertain) = context;
    let mut snapshot = authority.policy_snapshot(review, credential).await?;
    require_observed_submission(&snapshot, uncertain)?;
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
    uncertain: bool,
) -> Result<(), SourceDeliveryError> {
    if uncertain
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
    let submitted = api::command_json(
        mutation(authority, query, node_id, credential)?,
        authority.config.api_deadline,
        credential,
    )
    .await;
    if let Err(error) = &submitted {
        if error.cleanup_unconfirmed() {
            return Err(error.clone());
        }
    }
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

fn mutation(
    authority: &GhCliDeliveryAuthority,
    query: &str,
    node_id: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Command, SourceDeliveryError> {
    if node_id.trim().is_empty() {
        return Err(invalid_receipt());
    }
    let mut command = clean_command(&authority.config, &authority.config.gh_program, credential);
    command.args([
        "api",
        "graphql",
        "-f",
        &format!("query={query}"),
        "-f",
        &format!("pullRequestId={node_id}"),
    ]);
    Ok(command)
}

fn identity(review: &SourceReviewReceipt) -> QueueIdentity {
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

fn submission_panicked() -> SourceDeliveryError {
    SourceDeliveryError::Command(Box::new(crate::native_v2_delivery::GitCommandFailure {
        command: "gh api graphql enqueuePullRequest".into(),
        working_directory: "[PRIVATE]".into(),
        exit_status: None,
        stdout: String::new(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
        context: "merge queue submission panicked; process cleanup is unconfirmed".into(),
        timed_out: false,
        cleanup_unconfirmed: true,
        classification: None,
    }))
}
