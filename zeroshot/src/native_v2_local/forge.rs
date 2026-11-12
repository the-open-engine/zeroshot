//! Read-only local delivery admission, using the delivery factory and private shell snapshot.
use super::*;
use crate::source_code_provider::{SourceCapability, SourceDeliveryCredential};
use crate::source_code_provider::adapters::core::delivery_authority;

struct ForgeAccessRequest<'a> {
    submission: &'a RunSubmission,
    environment: &'a RunEnvironment,
    native_environment: &'a BTreeMap<String, String>,
    workspace: &'a Path,
    github_token: Option<&'a str>,
    github_token_file: Option<&'a TrustedGitHubTokenFile>,
    stopping: Option<tokio::sync::watch::Receiver<bool>>,
}

pub(crate) async fn check_forge_access(
    prepared: &PreparedLocalRun,
    state_root: &Path,
) -> Result<(), LocalCompositionError> {
    check_access(
        ForgeAccessRequest {
            submission: &prepared.submission,
            environment: &prepared.environment,
            native_environment: &prepared.native_environment,
            workspace: &prepared.workspace,
            github_token: prepared.github_token.as_deref(),
            github_token_file: prepared.github_token_file.as_ref(),
            // The invoking CLI owns this request; no detached controller exists to stop yet.
            stopping: None,
        },
        &state_root.join("forge-preflight"),
    )
    .await
}

/// Checkpoint restores resolve executables only after restoration under the controller lease.
/// The controller's stop request ends the request with confirmed cleanup, and a stopped check
/// is refused even when the forge answered.
pub(crate) async fn check_restored_forge_access(
    bootstrap: &crate::native_v2_portable_controller::PortableControllerBootstrap,
    stopping: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<(), LocalCompositionError> {
    if bootstrap.checkpoint.is_none() {
        return Ok(());
    }
    check_access(
        ForgeAccessRequest {
            submission: &bootstrap.submission,
            environment: &bootstrap.environment,
            native_environment: &bootstrap.native_environment,
            workspace: &bootstrap.workspace,
            github_token: bootstrap.github_token.as_deref(),
            github_token_file: bootstrap.github_token_file.as_ref(),
            stopping,
        },
        &bootstrap.storage.join("forge-preflight"),
    )
    .await
}

async fn check_access(
    prepared: ForgeAccessRequest<'_>,
    directory: &Path,
) -> Result<(), LocalCompositionError> {
    let required =
        crate::native_v2_admission::delivery_capabilities(&prepared.submission.graph.root);
    if !required.contains(&SourceCapability::PullRequest) {
        return Ok(());
    }
    let source = prepared
        .submission
        .source
        .as_ref()
        .ok_or(LocalCompositionError::DeliverySourceRequired)?;
    let mode = crate::native_v2_admission::delivery_mode(&prepared.submission.graph.root)
        .ok_or(LocalCompositionError::DeliverySourceRequired)?;
    let target = crate::native_v2_delivery::source_target(source, mode)
        .map_err(|_| LocalCompositionError::ResolvedSource)?;
    let identity = source
        .remote
        .as_ref()
        .and_then(|remote| remote.forge.as_ref());
    let forge = identity.map_or_else(
        || "GitHub".to_owned(),
        |identity| {
            format!(
                "{} ({})",
                identity.provider,
                identity
                    .base_url
                    .as_ref()
                    .map_or("default instance", |base| base.as_str())
            )
        },
    );
    let network = crate::source_code_provider::adapters::core::network::ForgeNetwork::from_snapshot(
        prepared.native_environment,
    );
    let failure = |step: &str, reason: String| {
        LocalCompositionError::ForgeAccess(network.redact(&format!(
            "{forge} access check failed at {step} for project {}: {reason}",
            target.repository
        )))
    };
    let programs = local_delivery_programs(
        source,
        &required,
        prepared.native_environment,
        prepared.workspace,
    )?;
    prepare_private_directory(directory)?;
    let authority = delivery_authority(
        SourceDeliveryFactoryRequest {
            source,
            workspace: prepared.workspace,
            git_program: &programs.git,
            gh_program: &programs.gh,
            home_directory: directory,
            private_directory: directory,
            git_identity: None,
            local_environment: Some(prepared.native_environment.clone()),
            required_capabilities: &required,
            operator_diagnostics: None,
        },
        None,
        None,
    )
    .map_err(|error| failure("client setup", error.to_string()))?;
    let field = authority
        .forge_credential_field()
        .ok_or_else(|| failure("authentication", "forge has no API credential field".into()))?;
    let binding = prepared
        .submission
        .runtime
        .nodes()
        .values()
        .find(|binding| {
            matches!(
                binding,
                crate::native_v2_contract::NodeRuntimeBinding::GitDelivery { .. }
            )
        })
        .ok_or(LocalCompositionError::DeliverySourceRequired)?;
    let environment = prepared.environment.resolve(binding).await?;
    let declared = crate::native_v2_delivery::forge_credential(&environment, field);
    let trusted = (field == "GH_TOKEN")
        .then_some(prepared.github_token)
        .flatten();
    let file = if declared.or(trusted).is_none() && field == "GH_TOKEN" {
        prepared
            .github_token_file
            .map(|file| file.read())
            .transpose()
            .map_err(|error| failure("authentication", error.to_string()))?
    } else {
        None
    };
    let token = declared.or(trusted).or(file.as_deref()).ok_or_else(|| {
        failure(
            "authentication",
            format!("required forge credential {field} was not supplied"),
        )
    })?;
    let step = "authenticated project/version read";
    let outcome = authority
        .check_access(
            &target.repository,
            SourceDeliveryCredential::token(token),
            prepared.stopping.clone(),
        )
        .await;
    if prepared
        .stopping
        .as_ref()
        .is_some_and(|request| *request.borrow())
    {
        // Stop intent wins a race with the answer: a cancelled check never admits the run.
        let unconfirmed = outcome
            .as_ref()
            .is_err_and(crate::source_code_provider::SourceDeliveryError::cleanup_unconfirmed);
        let reason = if unconfirmed {
            "forge access check cleanup could not be confirmed"
        } else {
            "forge access check was cancelled"
        };
        return Err(failure(step, reason.to_owned()));
    }
    outcome.map_err(|error| failure(step, error.to_string()))
}
