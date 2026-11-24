use openengine_cluster_protocol::{
    RunCheckpoint, RunCheckpointsParams, RunResumeFrom, RunStatus, TerminalResult,
};

use super::super::{CliRunStatus, NativeV2CliBackend, NativeV2CliError, RunResumeCommand, RunSelector};

pub(super) async fn apply_default(
    command: &mut RunResumeCommand,
    status: &CliRunStatus,
    backend: &impl NativeV2CliBackend,
) -> Result<(), NativeV2CliError> {
    if !command.api_default
        && command.from.is_none()
        && matches!(
            status,
            CliRunStatus::Target(RunStatus::Finished {
                terminal_result: TerminalResult::Failed { .. },
                ..
            })
        )
    {
        command.from = Some(newest_checkpoint(&command.run, backend).await?);
    }
    Ok(())
}

async fn newest_checkpoint(
    run: &RunSelector,
    backend: &impl NativeV2CliBackend,
) -> Result<RunResumeFrom, NativeV2CliError> {
    let mut after = None;
    let mut latest: Option<RunCheckpoint> = None;
    loop {
        let page = backend
            .run_checkpoints(
                run.target.as_deref(),
                RunCheckpointsParams {
                    run_id: run.run_id.clone(),
                    after,
                    limit: Some(openengine_cluster_protocol::MAX_CHECKPOINT_PAGE_SIZE),
                },
            )
            .await?;
        if let Some(checkpoint) = page.checkpoints.into_iter().last() {
            if latest
                .as_ref()
                .is_some_and(|prior| checkpoint.sequence <= prior.sequence)
            {
                return Err(NativeV2CliError::Target(
                    "checkpoint listing did not advance".to_owned(),
                ));
            }
            latest = Some(checkpoint);
        } else if page.next_after.is_some() {
            return Err(NativeV2CliError::Target(
                "checkpoint listing returned an empty continuation page".to_owned(),
            ));
        }
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    let checkpoint = latest.ok_or_else(|| {
        let route = match &run.target {
            Some(name) => format!(" --target {name}"),
            None if run.controller == super::super::local::LocalControllerMode::Contained =>
                " --controller contained".to_owned(),
            None => String::new(),
        };
        NativeV2CliError::Usage(format!(
            "run has no entry checkpoints; to restart where it began, run `zeroshot resume {}{route} --restart`",
            run.run_id.as_str(),
        ))
    })?;
    Ok(RunResumeFrom::Checkpoint {
        checkpoint_id: checkpoint.checkpoint_id,
    })
}
