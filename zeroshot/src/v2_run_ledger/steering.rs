//! Steering projections and publication share the run's sole vector record.
use std::collections::{BTreeSet, VecDeque};
use openengine_cluster_protocol::{
    NetworkSteer, NetworkSteerParams, RunId, RunSteer, RunSteerParams, RunSteerResult,
    RunSteeringStatus, RunVector, RunVectorContent, SteerOrigin,
};
use super::{RunEvent, RunLedger, RunLedgerError, vector_event};

pub(crate) async fn record(
    ledger: &dyn RunLedger,
    params: RunSteerParams,
) -> Result<RunSteerResult, RunLedgerError> {
    record_content(
        ledger,
        params.run_id,
        RunVectorContent::Steer {
            text: params.text,
            mode: params.mode,
            network: None,
        },
    )
    .await
}

pub(crate) async fn record_network(
    ledger: &dyn RunLedger,
    params: NetworkSteerParams,
    origin: SteerOrigin,
) -> Result<RunSteerResult, RunLedgerError> {
    let networks = params
        .networks
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    record_content(
        ledger,
        params.run_id,
        RunVectorContent::Steer {
            text: params.text,
            mode: params.mode,
            network: Some(NetworkSteer { networks, origin }),
        },
    )
    .await
}

async fn record_content(
    ledger: &dyn RunLedger,
    run_id: RunId,
    content: RunVectorContent,
) -> Result<RunSteerResult, RunLedgerError> {
    let event = vector_event(&run_id, content)?;
    let RunEvent::Vector { vector } = &event else {
        unreachable!()
    };
    vector
        .validate()
        .map_err(|error| match error.to_string().as_str() {
            "invalid steer text" => RunLedgerError::InvalidEvent("invalid steer text"),
            _ => RunLedgerError::InvalidEvent("invalid steer vector"),
        })?;
    let steer = summary(vector).ok_or(RunLedgerError::InvalidEvent("invalid steer vector"))?;
    ledger.append(&run_id, vec![event]).await?;
    Ok(RunSteerResult { run_id, steer })
}

#[cfg(test)]
pub(crate) async fn all(
    ledger: &dyn RunLedger,
    run: &RunId,
) -> Result<Vec<RunVector>, RunLedgerError> {
    let mut result = Vec::new();
    visit(ledger, run, |vector| {
        if matches!(
            vector.content,
            RunVectorContent::Steer { .. } | RunVectorContent::SteerReceipt { .. }
        ) {
            result.push(vector.clone());
        }
    })
    .await?;
    Ok(result)
}

async fn visit(
    ledger: &dyn RunLedger,
    run: &RunId,
    mut visitor: impl FnMut(&RunVector),
) -> Result<(), RunLedgerError> {
    let mut after = None;
    loop {
        let events = ledger.steering_events(run, after.as_ref()).await?;
        if events.is_empty() {
            return Ok(());
        }
        for stored in events {
            after = Some(stored.cursor);
            if let RunEvent::Vector { vector } = stored.event {
                visitor(&vector);
            }
        }
    }
}

pub(crate) fn summary(vector: &RunVector) -> Option<RunSteer> {
    let RunVectorContent::Steer {
        text,
        mode,
        network,
    } = &vector.content
    else {
        return None;
    };
    let mut end = text.len().min(1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Some(RunSteer {
        id: vector.id.clone(),
        mode: *mode,
        network: network.clone(),
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
    // Discard full texts as each bounded page is read. Only the last 64 excerpts survive.
    let mut total = 0_u64;
    let mut retained = VecDeque::new();
    visit(ledger, run, |vector| {
        if let Some(steer) = summary(vector) {
            total += 1;
            if retained.len() == 64 {
                retained.pop_front();
            }
            retained.push_back(steer);
        }
    })
    .await?;
    let mut steers: Vec<_> = retained.into_iter().collect();
    if !steers.is_empty() {
        let mut seen = BTreeSet::new();
        visit(ledger, run, |vector| {
            if let RunVectorContent::SteerReceipt {
                steer_id,
                execution,
                ..
            } = &vector.content
                && let Some(steer) = steers.iter_mut().find(|s| &s.id == steer_id)
                && seen.insert((steer_id.clone(), execution.clone()))
            {
                steer.received_executions += 1;
                if steer.executions.len() == 64 {
                    steer.executions.remove(0);
                    steer.executions_omitted = true;
                }
                steer.executions.push(execution.clone());
            }
        })
        .await?;
    }
    Ok(RunSteeringStatus {
        omitted_steers: total.saturating_sub(steers.len() as u64),
        steers,
    })
}
