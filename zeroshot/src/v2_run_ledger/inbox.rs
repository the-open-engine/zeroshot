//! Read-only projection of requests from each run's own vector record.
use super::*;
use openengine_cluster_protocol::{
    InboxEntry, InboxListParams, InboxListResult, InboxShowResult, RequestState, RunRequest,
    RunVectorContent, RunVectorsParams, UnixTimestampMillis,
};

/// A suspended snapshot retains its own request vector's bounded inbox fields.
pub fn entry(
    run_id: RunId,
    suspension: &openengine_cluster_protocol::RunSuspension,
    now: UnixTimestampMillis,
) -> Option<InboxEntry> {
    let openengine_cluster_protocol::RunSuspension::Request {
        node,
        created_at,
        request,
    } = suspension
    else {
        return None;
    };
    Some(InboxEntry {
        run_id,
        node: node.clone(),
        created_at: *created_at,
        expires_at: request.expires_at,
        state: if now >= request.expires_at {
            RequestState::Expired
        } else {
            RequestState::Pending
        },
    })
}

pub async fn show(
    ledger: &dyn RunLedger,
    run_id: &RunId,
    now: UnixTimestampMillis,
) -> Result<InboxShowResult, RunLedgerError> {
    let stored = ledger
        .get(run_id)
        .await?
        .ok_or(RunLedgerError::RunNotFound)?;
    let mut after = None;
    let mut own = None;
    loop {
        let page = super::vectors::list_full(
            ledger,
            RunVectorsParams {
                run_id: run_id.clone(),
                after,
                limit: Some(100),
            },
        )
        .await?;
        for vector in page.vectors {
            if &vector.run_id == run_id
                && let RunVectorContent::Request {
                    node,
                    text,
                    signals,
                    output,
                    expires_at,
                } = vector.content
            {
                own = Some(InboxShowResult {
                    run_id: run_id.clone(),
                    node,
                    created_at: vector.created_at,
                    request: RunRequest {
                        id: vector.id,
                        text,
                        signals,
                        output,
                        expires_at,
                    },
                    state: if stored.snapshot.phase == RunPhase::Finished {
                        RequestState::Stopped
                    } else if now >= expires_at {
                        RequestState::Expired
                    } else {
                        RequestState::Pending
                    },
                    successor_run_id: None,
                });
            }
        }
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    own.ok_or(RunLedgerError::VectorNotFound)
}

pub async fn with_successor(
    ledger: &dyn RunLedger,
    mut request: InboxShowResult,
    successor: RunId,
) -> Result<InboxShowResult, RunLedgerError> {
    // Workspace ownership can publish the successor before its admission reaches the ledger.
    request.successor_run_id = Some(successor.clone());
    if ledger.get(&successor).await?.is_none() {
        return Ok(request);
    }
    let mut after = None;
    loop {
        let page = super::vectors::list_full(
            ledger,
            RunVectorsParams {
                run_id: successor.clone(),
                after,
                limit: Some(100),
            },
        )
        .await?;
        for vector in page.vectors {
            match vector.content {
                RunVectorContent::Answer { request_id, .. } if request_id == request.request.id => {
                    request.state = RequestState::Answered;
                }
                RunVectorContent::Expiry { request_id } if request_id == request.request.id => {
                    request.state = RequestState::Expired;
                }
                _ => {}
            }
        }
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    Ok(request)
}

pub fn page(
    mut requests: Vec<InboxEntry>,
    params: InboxListParams,
) -> Result<InboxListResult, RunLedgerError> {
    params
        .validate()
        .map_err(|_| RunLedgerError::InvalidEvent("inbox limit must be 1 through 100"))?;
    requests.sort_by(|a, b| a.run_id.as_str().cmp(b.run_id.as_str()));
    requests.retain(|entry| {
        params
            .after
            .as_ref()
            .is_none_or(|after| entry.run_id.as_str() > after.as_str())
    });
    let limit = params.limit.unwrap_or(50) as usize;
    let more = requests.len() > limit;
    requests.truncate(limit);
    let next_after = if more {
        requests.last().map(|entry| entry.run_id.clone())
    } else {
        None
    };
    Ok(InboxListResult {
        requests,
        next_after,
    })
}
