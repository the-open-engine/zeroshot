//! The narrow readmission exception for a confirmed queue-order conflict.
use super::*;
use std::collections::BTreeSet;

const MAX_CONFLICT_RECOVERIES: usize = 3;

pub(in super::super) struct RecoveryPolicy {
    clean: bool,
    entries: Vec<(String, time::OffsetDateTime, bool)>,
}

pub(in super::super) fn recovery_policy(
    value: &Value,
    review: &SourceReviewReceipt,
) -> Result<Option<RecoveryPolicy>, SourceDeliveryError> {
    let Some(pages) = value.as_array() else {
        return Ok(None);
    };
    let Some(first) = pages.first() else {
        return Ok(None);
    };
    let repository = &first["data"]["repository"];
    let queue = &repository["mergeQueue"];
    if queue.is_null()
        || pages
            .iter()
            .any(|page| page["data"]["repository"]["mergeQueue"] != *queue)
    {
        return Ok(None);
    }
    let connection = &queue["entries"];
    let Some(nodes) = connection["nodes"].as_array() else {
        return Ok(None);
    };
    // This bounded independent connection never shares the required-check paginator.
    // A truncated/changing queue cannot prove that predecessors have drained.
    if queue["id"].as_str().is_none_or(|id| id.trim().is_empty())
        || connection.pointer("/queuePageInfo/hasNextPage") != Some(&Value::Bool(false))
        || connection["totalCount"].as_u64() != Some(nodes.len() as u64)
        || nodes.len() > 100
    {
        return Ok(None);
    }
    let mut identities = BTreeSet::new();
    let mut entries = Vec::new();
    for node in nodes {
        let pull = &node["pullRequest"];
        let id = pull["id"].as_str().filter(|id| !id.trim().is_empty());
        let entry_id = node["id"].as_str().filter(|id| !id.trim().is_empty());
        let timestamp = queue_timestamp(&node["enqueuedAt"]);
        let jump = node["jump"].as_bool();
        let (Some(id), Some(entry_id), Some(timestamp), Some(jump)) =
            (id, entry_id, timestamp, jump)
        else {
            return Ok(None);
        };
        if pull["baseRefName"].as_str() != Some(&review.target_branch)
            || pull
                .pointer("/repository/nameWithOwner")
                .and_then(Value::as_str)
                != Some(&review.repository)
            || !identities.insert(entry_id)
        {
            return Err(SourceDeliveryError::identity(
                "Merge queue entries changed repository, base or entry identity",
            ));
        }
        entries.push((id.to_owned(), timestamp, jump));
    }
    Ok(Some(RecoveryPolicy {
        clean: repository["pullRequest"]["mergeable"] == "MERGEABLE",
        entries,
    }))
}

pub(super) async fn recover(
    authority: &GhCliDeliveryAuthority,
    review: &SourceReviewReceipt,
    snapshot: &mut PolicySnapshot,
    removed: (QueueAdmission, String),
) -> Result<(), SourceDeliveryError> {
    let (admission, removal_id) = removed;
    let mut effects = authority.queue_effects.0.lock().await;
    let Some(effect) = effects.get_mut(&identity(review)) else {
        return pending(snapshot);
    };
    if effect
        .settled
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        != Some(&admission)
        || effect.recovered_removals.contains(&removal_id)
    {
        return pending(snapshot);
    }
    effect.recoverable = None;
    if effect.uncertain {
        return Err(SourceDeliveryError::api(
            None,
            "Merge queue conflict cannot release an earlier unconfirmed submission",
        ));
    }
    let Some(policy) = &snapshot.queue_recovery else {
        return pending(snapshot);
    };
    if policy.entries.iter().any(|(id, timestamp, jump)| {
        id == &admission.pull_request_id || *timestamp <= admission.enqueued_at || *jump
    }) {
        return pending(snapshot);
    }
    match &snapshot.state {
        SourceReviewState::Conflict => return Ok(()),
        SourceReviewState::Open {
            checks: SourceChecks::Failed { .. },
        } => return Ok(()),
        _ => {}
    }
    if !policy.clean || !snapshot.is_merge_queue_enabled {
        return pending(snapshot);
    }
    if matches!(
        snapshot.state,
        SourceReviewState::Open {
            checks: SourceChecks::Passed | SourceChecks::NotRequired
        }
    ) {
        if effect.recovered_removals.len() >= MAX_CONFLICT_RECOVERIES {
            return Err(SourceDeliveryError::api(None, "Merge queue merge_conflict recovery allowance exhausted after three readmissions; inspect queue ordering before retrying delivery").exhausted());
        }
        effect.recoverable = Some(removal_id);
    }
    Ok(())
}

pub(super) fn pending(snapshot: &mut PolicySnapshot) -> Result<(), SourceDeliveryError> {
    snapshot.state = SourceReviewState::Open {
        checks: SourceChecks::Pending,
    };
    snapshot.failed_job_ids.clear();
    snapshot.head_update = None;
    snapshot.pull_request_ready = false;
    Ok(())
}
