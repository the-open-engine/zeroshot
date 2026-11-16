//! Capability-routed node publication bypasses local and named controller discovery.
use std::ffi::OsString;
use std::path::PathBuf;

use openengine_cluster_protocol::{NetworkSteerParams, RunSteerResult};
use super::{NativeV2CliCommand, NativeV2CliError, read_steer_text};

pub(super) async fn try_publish(
    command: &NativeV2CliCommand,
    environment: &dyn Fn(&str) -> Option<OsString>,
) -> Result<Option<RunSteerResult>, NativeV2CliError> {
    let NativeV2CliCommand::Steer {
        run,
        text,
        networks,
        mode,
    } = command
    else {
        return Ok(None);
    };
    let capability = environment("ZEROSHOT_STEERING_CAPABILITY");
    if networks.is_empty() {
        if capability.is_some() || environment("ZEROSHOT_EXECUTION_REF").is_some() {
            return Err(NativeV2CliError::Usage(
                "node messages require an authorized --network".into(),
            ));
        }
        return Ok(None);
    }
    if run.target.is_some() {
        return Err(NativeV2CliError::Usage(
            "network messages cannot use --target".into(),
        ));
    }
    let path = capability.map(PathBuf::from).ok_or_else(|| {
        NativeV2CliError::Usage("this node has no network publishing mandate".into())
    })?;
    let params = NetworkSteerParams {
        run_id: run.run_id.clone(),
        text: read_steer_text(text.clone())?,
        networks: networks.clone(),
        mode: *mode,
    };
    let result = tokio::task::spawn_blocking(move || {
        crate::active_memory::hooks::publish_network(&path, params)
    })
    .await
    .map_err(|error| NativeV2CliError::Local(format!("network publication failed: {error}")))?
    .map_err(|error| NativeV2CliError::Local(format!("network publication failed: {error}")))?;
    Ok(Some(result))
}

pub(super) async fn execute<B: super::NativeV2CliBackend>(
    command: NativeV2CliCommand,
    context: &super::CliExecutionContext<'_, B>,
    output: &mut impl std::io::Write,
) -> Result<super::CliOutcome, NativeV2CliError> {
    let result = if let Some(result) = try_publish(&command, context.environment).await? {
        result
    } else {
        let NativeV2CliCommand::Steer {
            run, text, mode, ..
        } = command
        else {
            return Err(NativeV2CliError::Usage("expected a steer command".into()));
        };
        context
            .backend
            .run_steer(
                run.target.as_deref(),
                openengine_cluster_protocol::RunSteerParams {
                    run_id: run.run_id,
                    text: read_steer_text(text)?,
                    mode,
                },
            )
            .await?
    };
    super::write_json(output, &result)?;
    Ok(super::CliOutcome::Completed)
}
