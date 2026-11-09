//! A removed queue candidate is feedback, never permission to silently enqueue it again.
use super::*;
use std::collections::BTreeSet;

const OUTCOME_QUERY: &str = r#"
query QueueOutcome($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    nameWithOwner
    pullRequest(number: $number) {
      id number baseRefName headRefName headRefOid
      timelineItems(last: 1, itemTypes: [REMOVED_FROM_MERGE_QUEUE_EVENT]) {
        nodes {
          __typename
          ... on RemovedFromMergeQueueEvent {
            id reason createdAt
            beforeCommit {
              oid
              statusCheckRollup {
                contexts(first: 100, after: $endCursor) {
                  pageInfo { hasNextPage endCursor }
                  nodes {
                    __typename
                    ... on CheckRun {
                      name status conclusion detailsUrl databaseId
                      checkSuite { app { databaseId } }
                      isRequired(pullRequestNumber: $number)
                    }
                    ... on StatusContext {
                      context state description targetUrl
                      isRequired(pullRequestNumber: $number)
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"#;

pub(in super::super) async fn observe_removal(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    snapshot: &mut PolicySnapshot,
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    if snapshot.is_in_merge_queue || !matches!(snapshot.state, SourceReviewState::Open { .. }) {
        return Ok(());
    }
    let admission = {
        let effects = authority.queue_effects.0.lock().await;
        effects
            .get(&identity(review))
            .and_then(|effect| effect.settled.as_ref())
            .and_then(|result| result.as_ref().ok())
            .cloned()
    };
    let Some(admission) = admission else {
        return Ok(());
    };
    let value = authority
        .api(&review_query_arguments(review, OUTCOME_QUERY)?, credential)
        .await?;
    if let Some((diagnostic, jobs)) = removed_candidate(value, review, &admission)? {
        snapshot.state = SourceReviewState::Open {
            checks: SourceChecks::Failed { diagnostic },
        };
        snapshot.failed_job_ids = jobs;
        snapshot.pull_request_ready = false;
    }
    Ok(())
}

#[derive(Eq, PartialEq)]
struct Removal {
    id: String,
    reason: String,
    revision: Option<String>,
}

fn removed_candidate(
    value: Value,
    review: &SourceReviewReceipt,
    admission: &QueueAdmission,
) -> Result<Option<(String, Vec<u64>)>, SourceDeliveryError> {
    let pages = value.as_array().ok_or_else(invalid_outcome)?;
    let mut identity = None;
    let mut contexts = Vec::new();
    let mut cursors = BTreeSet::new();
    for (index, page) in pages.iter().enumerate() {
        let pull = outcome_pull_request(page, review, &admission.pull_request_id)?;
        let Some(removed) = matching_removal(pull, admission)? else {
            return Ok(None);
        };
        let current = removal_identity(removed)?;
        if identity
            .as_ref()
            .is_some_and(|previous| previous != &current)
        {
            return Err(invalid_outcome().temporary());
        }
        identity = Some(current);
        contexts.extend(candidate_checks(
            removed,
            index + 1 == pages.len(),
            pages.len() == 1,
            &mut cursors,
        )?);
    }
    removal_feedback(identity.ok_or_else(invalid_outcome)?, contexts).map(Some)
}

fn removal_feedback(
    removal: Removal,
    contexts: Vec<Value>,
) -> Result<(String, Vec<u64>), SourceDeliveryError> {
    let (checks, jobs) = policy::queue_check_failures(Value::Array(contexts))?;
    let diagnostic = format!(
        "Merge queue removed candidate {} before merge ({}). Reassess the queue failure before requesting another admission.\n{checks}",
        removal
            .revision
            .as_deref()
            .unwrap_or("without a published queue revision"),
        removal.reason,
    );
    Ok((diagnostic, jobs))
}

fn outcome_pull_request<'a>(
    page: &'a Value,
    review: &SourceReviewReceipt,
    node_id: &str,
) -> Result<&'a Value, SourceDeliveryError> {
    let repository = &page["data"]["repository"];
    let pull = &repository["pullRequest"];
    if repository["nameWithOwner"].as_str() != Some(&review.repository)
        || pull["number"]
            .as_u64()
            .map(|number| number.to_string())
            .as_deref()
            != Some(&review.review_id)
        || pull["baseRefName"].as_str() != Some(&review.target_branch)
        || pull["headRefName"].as_str() != Some(&review.head_branch)
        || pull["headRefOid"].as_str() != Some(&review.head_revision)
        || pull["id"].as_str() != Some(node_id)
    {
        return Err(SourceDeliveryError::identity(
            "Merge queue outcome changed the admitted PR identity",
        ));
    }
    Ok(pull)
}

fn matching_removal<'a>(
    pull: &'a Value,
    admission: &QueueAdmission,
) -> Result<Option<&'a Value>, SourceDeliveryError> {
    let events = pull
        .pointer("/timelineItems/nodes")
        .and_then(Value::as_array)
        .ok_or_else(invalid_outcome)?;
    let [removed] = events.as_slice() else {
        return Ok(None);
    };
    if removed["__typename"] != "RemovedFromMergeQueueEvent" {
        return Err(invalid_outcome());
    }
    let removed_at = queue_timestamp(&removed["createdAt"]).ok_or_else(invalid_outcome)?;
    // GitHub timestamps have second precision. Equality conservatively requests feedback, never merge.
    Ok((removed_at >= admission.enqueued_at && removed["reason"] != "merged").then_some(removed))
}

fn removal_identity(removed: &Value) -> Result<Removal, SourceDeliveryError> {
    let revision = removed.pointer("/beforeCommit/oid").and_then(Value::as_str);
    if revision.is_some_and(|revision| !valid_revision(revision)) {
        return Err(invalid_outcome());
    }
    let id = removed["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(invalid_outcome)?;
    Ok(Removal {
        id: id.to_owned(),
        reason: removed["reason"]
            .as_str()
            .unwrap_or("unknown removal reason")
            .to_owned(),
        revision: revision.map(str::to_owned),
    })
}

fn candidate_checks(
    removed: &Value,
    last: bool,
    single: bool,
    cursors: &mut BTreeSet<String>,
) -> Result<Vec<Value>, SourceDeliveryError> {
    match removed
        .pointer("/beforeCommit/statusCheckRollup/contexts")
        .filter(|connection| !connection.is_null())
    {
        Some(connection) => checked_contexts(connection, last, cursors),
        None if single => Ok(Vec::new()),
        None => Err(invalid_outcome()),
    }
}

fn checked_contexts(
    connection: &Value,
    last: bool,
    cursors: &mut BTreeSet<String>,
) -> Result<Vec<Value>, SourceDeliveryError> {
    let has_next = connection
        .pointer("/pageInfo/hasNextPage")
        .and_then(Value::as_bool)
        .ok_or_else(invalid_outcome)?;
    let cursor = connection
        .pointer("/pageInfo/endCursor")
        .and_then(Value::as_str);
    if has_next == last
        || has_next && cursor.is_none()
        || cursor.is_some_and(|cursor| cursor.is_empty() || !cursors.insert(cursor.to_owned()))
    {
        return Err(invalid_outcome());
    }
    connection["nodes"]
        .as_array()
        .cloned()
        .ok_or_else(invalid_outcome)
}

fn invalid_outcome() -> SourceDeliveryError {
    SourceDeliveryError::api(
        None,
        "GitHub returned incomplete or inconsistent merge queue outcome evidence",
    )
}
