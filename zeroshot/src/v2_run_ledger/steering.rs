//! Steering projections and publication share the run's sole vector record.
use std::collections::BTreeSet;
use openengine_cluster_protocol::{
    RunId, RunSteer, RunSteerParams, RunSteerResult, RunSteeringStatus, RunVector,
    RunVectorContent, RunVectorsParams,
};
use super::{RunEvent, RunLedger, RunLedgerError, vector_event};

pub(crate) async fn record(
    ledger: &dyn RunLedger,
    params: RunSteerParams,
) -> Result<RunSteerResult, RunLedgerError> {
    openengine_cluster_protocol::validate_steer(&params.text)
        .map_err(|_| RunLedgerError::InvalidEvent("invalid steer text"))?;
    let event = vector_event(
        &params.run_id,
        RunVectorContent::Steer { text: params.text },
    )?;
    let RunEvent::Vector { vector } = &event else {
        unreachable!()
    };
    let steer = summary(vector).ok_or(RunLedgerError::InvalidEvent("invalid steer vector"))?;
    ledger.append(&params.run_id, vec![event]).await?;
    Ok(RunSteerResult {
        run_id: params.run_id,
        steer,
    })
}

pub(crate) async fn all(
    ledger: &dyn RunLedger,
    run: &RunId,
) -> Result<Vec<RunVector>, RunLedgerError> {
    let mut result = Vec::new();
    let mut after = None;
    loop {
        let page = super::vectors::list_full(
            ledger,
            RunVectorsParams {
                run_id: run.clone(),
                after,
                limit: Some(100),
            },
        )
        .await?;
        result.extend(page.vectors.into_iter().filter(|v| {
            matches!(
                v.content,
                RunVectorContent::Steer { .. } | RunVectorContent::SteerReceipt { .. }
            )
        }));
        after = page.next_after;
        if after.is_none() {
            return Ok(result);
        }
    }
}

pub(crate) fn summary(vector: &RunVector) -> Option<RunSteer> {
    let RunVectorContent::Steer { text } = &vector.content else {
        return None;
    };
    let mut end = text.len().min(1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(RunSteer {
        id: vector.id.clone(),
        created_at: vector.created_at,
        text_bytes: text.len(),
        excerpt: text[..end].into(),
        received_executions: 0,
        executions: Vec::new(),
        executions_omitted: false,
    })
}

pub(crate) async fn status(
    ledger: &dyn RunLedger,
    run: &RunId,
) -> Result<RunSteeringStatus, RunLedgerError> {
    // At most 64 summaries with 64 opaque execution references fit comfortably in one frame.
    let vectors = all(ledger, run).await?;
    let total = vectors
        .iter()
        .filter(|v| matches!(v.content, RunVectorContent::Steer { .. }))
        .count();
    let mut steers: Vec<_> = vectors.iter().filter_map(summary).rev().take(64).collect();
    steers.reverse();
    let mut seen = BTreeSet::new();
    for vector in &vectors {
        if let RunVectorContent::SteerReceipt {
            steer_id,
            execution,
            ..
        } = &vector.content
            && seen.insert((steer_id, execution))
            && let Some(steer) = steers.iter_mut().find(|s| &s.id == steer_id)
        {
            steer.received_executions += 1;
            if steer.executions.len() == 64 {
                steer.executions.remove(0);
                steer.executions_omitted = true;
            }
            steer.executions.push(execution.clone());
        }
    }
    Ok(RunSteeringStatus {
        omitted_steers: total.saturating_sub(steers.len()) as u64,
        steers,
    })
}
