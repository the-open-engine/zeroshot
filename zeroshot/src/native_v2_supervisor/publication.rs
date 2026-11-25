//! Check complete publication acknowledgements before the host imports any bytes.
use super::*;
use crate::native_v2_files::publication::PublicationPreflight;
use crate::v2_run_ledger::{PrimaryGraphCompletion, validate_event};
use openengine_cluster_protocol::{
    InboxShowResult, PublishedFiles, RequestState, RunMetadata, RunStatus, RunStatusResult,
    RunSuspension, RunVector, RunVectorContent, MAX_REQUEST_RESPONSE_BYTES,
};

fn record(event: &RunEvent) -> std::io::Result<()> {
    validate_event(event).map_err(|error| {
        std::io::Error::other(format!("artifact publication acknowledgement: {error}"))
    })
}

fn response(value: &impl serde::Serialize) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(std::io::Error::other)?
        .len();
    if bytes > MAX_REQUEST_RESPONSE_BYTES {
        return Err(std::io::Error::other(format!(
            "artifact publication response is {bytes} bytes, exceeding {MAX_REQUEST_RESPONSE_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(super) fn final_preflight(
    snapshot: &RunSnapshot,
    primary: &PrimaryGraphCompletion,
) -> Result<PublicationPreflight, NativeV2SupervisorError> {
    for event in logging::primary_graph_events(primary.clone())? {
        record(&event).map_err(NativeV2SupervisorError::FilePublication)?;
    }
    record(&RunEvent::Terminal {
        result: primary.result.clone(),
    })
    .map_err(NativeV2SupervisorError::FilePublication)?;
    let status = RunStatusResult {
        steering: Default::default(),
        run_id: snapshot.run_id.clone(),
        title: snapshot.title.clone(),
        source: snapshot.source.clone(),
        context_sources: snapshot.context_sources.clone(),
        size: snapshot.size.clone(),
        // Later finalization may advance the cursor; reserve its full existing integer width.
        at_cursor: crate::v2_run_ledger::cursor_for(u64::MAX),
        status: RunStatus::Finished {
            terminal_result: primary.result.clone(),
            metadata: RunMetadata {
                artifacts: Default::default(),
                token_usage: snapshot.token_usage.clone(),
                delivery_receipt: primary
                    .delivery_execution
                    .and_then(|id| snapshot.execution_output(id))
                    .cloned(),
            },
        },
        controller: None,
        workspace_recovery: Default::default(),
    };
    Ok(Arc::new(move |files: &PublishedFiles| {
        record(&RunEvent::FinalArtifactsPublished {
            artifacts: files.clone(),
        })?;
        let mut status = status.clone();
        let RunStatus::Finished { metadata, .. } = &mut status.status else {
            unreachable!()
        };
        metadata.artifacts = files.clone();
        response(&status)
    }))
}

pub(super) fn request_vector(run_id: &RunId, suspension: &RunSuspension) -> Option<RunEvent> {
    let RunSuspension::Request {
        node,
        created_at,
        request,
    } = suspension
    else {
        return None;
    };
    Some(RunEvent::Vector {
        vector: RunVector {
            id: request.id.clone(),
            run_id: run_id.clone(),
            created_at: *created_at,
            content: RunVectorContent::Request {
                artifacts: request.artifacts.clone(),
                node: node.clone(),
                text: request.text.clone(),
                signals: request.signals.clone(),
                output: request.output.clone(),
                expires_at: request.expires_at,
            },
        },
    })
}

pub(super) fn request_preflight(
    run_id: &RunId,
    suspension: &RunSuspension,
) -> PublicationPreflight {
    let run_id = run_id.clone();
    let suspension = suspension.clone();
    Arc::new(move |files| {
        let mut suspension = suspension.clone();
        let RunSuspension::Request {
            node,
            created_at,
            request,
        } = &mut suspension
        else {
            return Err(std::io::Error::other(
                "request publication requires a request",
            ));
        };
        request.artifacts = files.clone();
        response(&InboxShowResult {
            run_id: run_id.clone(),
            node: node.clone(),
            created_at: *created_at,
            request: *request.clone(),
            state: RequestState::Pending,
            successor_run_id: None,
        })?;
        record(&request_vector(&run_id, &suspension).expect("request publication"))?;
        record(&RunEvent::Suspended { suspension })
    })
}
