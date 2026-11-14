//! Vector pages replay ledger events without changing durable state.
use super::*;
use openengine_cluster_protocol::{RunVector, RunVectorParams, RunVectorsParams, RunVectorsResult};

#[derive(Debug, PartialEq)]
pub struct FullVectorPage {
    pub run_id: RunId,
    pub vectors: Vec<RunVector>,
    pub next_after: Option<openengine_cluster_protocol::VectorId>,
}

pub async fn list(
    ledger: &dyn RunLedger,
    params: RunVectorsParams,
) -> Result<RunVectorsResult, RunLedgerError> {
    let page = list_full(ledger, params).await?;
    Ok(RunVectorsResult {
        run_id: page.run_id,
        vectors: page.vectors.into_iter().map(Into::into).collect(),
        next_after: page.next_after,
    })
}

pub async fn list_full(
    ledger: &dyn RunLedger,
    params: RunVectorsParams,
) -> Result<FullVectorPage, RunLedgerError> {
    params
        .validate()
        .map_err(|_| RunLedgerError::InvalidEvent("vector limit must be 1 through 100"))?;
    let mut after = match params.after.as_ref() {
        Some(id) => Some(ledger.vector_cursor(&params.run_id, id).await?),
        None => None,
    };
    let mut vectors = Vec::new();
    let limit = params.limit.unwrap_or(50) as usize;
    loop {
        let tail = ledger
            .snapshot_and_tail(&params.run_id, after.as_ref())
            .await?;
        for stored in tail.events {
            after = Some(stored.cursor);
            if let RunEvent::Vector { vector } = stored.event {
                if vectors.len() == limit {
                    let next_after = vectors.last().map(|v: &RunVector| v.id.clone());
                    return Ok(FullVectorPage {
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
    Ok(FullVectorPage {
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
        let page = list_full(
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
