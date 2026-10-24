use openengine_cluster_protocol::{
    GitTransportCredentials, ResolvedSource, SourceBranchId, SourceRevisionId,
};
use serde::Deserialize;
use crate::native_v2_contract::AdmittedRun;
use crate::native_v2_portable_controller::{PortableControllerBootstrap, PortableControllerError};

use super::{ReviewResolveError, ReviewTargetKind, parse_review_url};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckoutScope {
    base_repository: String,
    base_branch: SourceBranchId,
    base_revision: SourceRevisionId,
    head_revision: SourceRevisionId,
}

/// Exact extra objects for a prepared PR review, fetched from its base repository.
pub(crate) fn checkout_references(
    admitted: &AdmittedRun,
) -> Result<Vec<String>, ReviewResolveError> {
    let Some(input) = crate::native_v2_admission::admitted_comment_input(admitted)
        .map_err(|_| ReviewResolveError::Identity)?
    else {
        return Ok(Vec::new());
    };
    let Some(scope) = input.review_scope else {
        return Ok(Vec::new());
    };
    if scope.as_object().is_some_and(serde_json::Map::is_empty) {
        return Ok(Vec::new());
    }
    let scope: CheckoutScope =
        serde_json::from_value(scope).map_err(|_| ReviewResolveError::Identity)?;
    let target = parse_review_url(&input.url)?;
    let Some(source) = admitted.source.as_ref() else {
        return Ok(Vec::new());
    };
    validate_scope(&scope, source, &target)?;
    Ok(vec![
        format!("refs/pull/{}/head", target.number),
        scope.base_revision.as_str().to_owned(),
    ])
}

/// Runs only after the local controller owns the workspace and has restored any checkpoint.
pub(crate) async fn prepare_local_checkout(
    admitted: &AdmittedRun,
    bootstrap: &PortableControllerBootstrap,
) -> Result<(), PortableControllerError> {
    use crate::native_v2_delivery::transport::{GitTransport, GitTransportConfig, source_credential};
    let references = checkout_references(admitted)
        .map_err(|error| PortableControllerError::Io(std::io::Error::other(error.to_string())))?;
    if references.is_empty() {
        return Ok(());
    }
    let source = admitted
        .source
        .as_ref()
        .ok_or(PortableControllerError::Workspace)?;
    let program = crate::execution::platform::find_executable(
        "git",
        &bootstrap.native_environment,
        &bootstrap.workspace,
    )
    .ok_or_else(|| {
        PortableControllerError::Io(std::io::Error::other("review checkout requires Git"))
    })?;
    let (token, credentials) = local_checkout_authority(admitted, bootstrap).await?;
    let private = bootstrap.storage.join("review-source");
    crate::execution::platform::private_directory(&private).map_err(PortableControllerError::Io)?;
    let transport = GitTransport::new(
        GitTransportConfig {
            git_program: program,
            workspace: bootstrap.workspace.clone(),
            home_directory: bootstrap
                .native_environment
                .get("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| private.clone()),
            private_directory: private,
            identity: None,
            local_environment: Some(bootstrap.native_environment.clone()),
            deadline: super::LOCAL_CHECKOUT_TIMEOUT,
        },
        crate::source_code_provider::adapters::core::source_remote(source),
    );
    transport
        .fetch_review_references(
            &references,
            source_credential(token.as_deref().unwrap_or(""), credentials.as_ref()),
        )
        .await
        .map_err(|_| {
            PortableControllerError::Io(std::io::Error::other(
                "could not fetch captured PR head/base for review",
            ))
        })
}

async fn local_checkout_authority(
    admitted: &AdmittedRun,
    bootstrap: &PortableControllerBootstrap,
) -> Result<(Option<String>, Option<GitTransportCredentials>), PortableControllerError> {
    use crate::native_v2_delivery::{github_git_token, resolved_git_credentials};
    let publisher = crate::native_v2_admission::comment_node(&admitted.graph.root)
        .ok_or(PortableControllerError::RuntimeUnavailable)?;
    let binding = admitted
        .runtime
        .nodes()
        .get(&publisher.name)
        .filter(|binding| {
            matches!(
                binding,
                crate::native_v2_contract::NodeRuntimeBinding::CommentDelivery { .. }
            )
        })
        .ok_or(PortableControllerError::RuntimeUnavailable)?;
    let environment = bootstrap
        .environment
        .resolve(binding)
        .await?
        .with_source_git_credentials(bootstrap.environment.resolve_git_source().await?);
    let invalid = |_| {
        PortableControllerError::Io(std::io::Error::other(
            "review checkout credentials are unavailable or invalid",
        ))
    };
    let token = github_git_token(&environment, bootstrap.github_token.as_deref())
        .map_err(invalid)?
        .map(str::to_owned);
    let credentials = resolved_git_credentials(&environment, bootstrap.git_credentials.as_ref())
        .map_err(invalid)?;
    let token = if token.is_none() && credentials.is_none() {
        bootstrap
            .github_token_file
            .as_ref()
            .map(|file| file.read())
            .transpose()
            .map_err(invalid)?
    } else {
        token
    };
    Ok((token, credentials))
}

fn validate_scope(
    scope: &CheckoutScope,
    source: &ResolvedSource,
    target: &super::ReviewTarget,
) -> Result<(), ReviewResolveError> {
    if target.kind != ReviewTargetKind::PullRequest
        || !target
            .repository
            .eq_ignore_ascii_case(source.repository.as_str())
        || !scope
            .base_repository
            .eq_ignore_ascii_case(source.repository.as_str())
        || scope.base_branch != source.branch
        || scope.head_revision != source.revision
    {
        return Err(ReviewResolveError::Identity);
    }
    Ok(())
}

#[cfg(test)]
#[path = "checkout/tests.rs"]
mod tests;
