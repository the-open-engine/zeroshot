//! Vector pages replay ledger events without changing durable state.
use super::*;
use openengine_cluster_protocol::{RunVector, RunVectorParams, RunVectorsParams, RunVectorsResult};

pub async fn list(
    ledger: &dyn RunLedger,
    params: RunVectorsParams,
) -> Result<RunVectorsResult, RunLedgerError> {
    params
        .validate()
        .map_err(|_| RunLedgerError::InvalidEvent("vector limit must be 1 through 100"))?;
    let mut after = None;
    let mut found = params.after.is_none();
    let mut vectors = Vec::new();
    let limit = params.limit.unwrap_or(50) as usize;
    loop {
        let tail = ledger
            .snapshot_and_tail(&params.run_id, after.as_ref())
            .await?;
        for stored in tail.events {
            after = Some(stored.cursor);
            if let RunEvent::Vector { vector } = stored.event {
                if !found {
                    found = params.after.as_ref() == Some(&vector.id);
                    continue;
                }
                if vectors.len() == limit {
                    let next_after = vectors.last().map(|v: &RunVector| v.id.clone());
                    return Ok(RunVectorsResult {
                        run_id: params.run_id,
                        vectors,
                        next_after,
                    });
                }
                vectors.push(vector);
            }
        }
        if after.as_ref() == Some(&tail.snapshot.cursor)
            || tail.snapshot.cursor.as_str() == INITIAL_CURSOR
        {
            break;
        }
    }
    if !found {
        return Err(RunLedgerError::VectorNotFound);
    }
    Ok(RunVectorsResult {
        run_id: params.run_id,
        vectors,
        next_after: None,
    })
}

pub async fn show(
    ledger: &dyn RunLedger,
    params: RunVectorParams,
) -> Result<RunVector, RunLedgerError> {
    let mut after = None;
    loop {
        let tail = ledger
            .snapshot_and_tail(&params.run_id, after.as_ref())
            .await?;
        for stored in tail.events {
            after = Some(stored.cursor);
            if let RunEvent::Vector { vector } = stored.event
                && vector.id == params.id
            {
                return Ok(vector);
            }
        }
        if after.as_ref() == Some(&tail.snapshot.cursor)
            || tail.snapshot.cursor.as_str() == INITIAL_CURSOR
        {
            return Err(RunLedgerError::VectorNotFound);
        }
    }
}

/// Copies the predecessor's entries verbatim and appends the successor's continuation.
pub async fn inherit(
    source: &dyn RunLedger,
    destination: &dyn RunLedger,
    predecessor: &RunId,
    successor: &RunId,
) -> Result<(), RunLedgerError> {
    let mut after = None;
    loop {
        let page = list(
            source,
            RunVectorsParams {
                run_id: predecessor.clone(),
                after,
                limit: Some(100),
            },
        )
        .await?;
        after = page.next_after;
        if !page.vectors.is_empty() {
            destination
                .append(
                    successor,
                    page.vectors
                        .into_iter()
                        .map(|vector| RunEvent::Vector { vector })
                        .collect(),
                )
                .await?;
        }
        if after.is_none() {
            break;
        }
    }
    destination
        .append(
            successor,
            vec![vector_event(
                successor,
                openengine_cluster_protocol::RunVectorContent::Continuation {
                    resumed_from: predecessor.clone(),
                },
            )?],
        )
        .await?;
    Ok(())
}
