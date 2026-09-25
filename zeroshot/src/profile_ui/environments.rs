//! Resource management stays with the authoring store, separate from accepted runs.
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use openengine_cluster_protocol::{
    EnvironmentId, EnvironmentRevision, RuntimeEnvironmentDeleteRequest,
    RuntimeEnvironmentSaveRequest,
};
use serde::Deserialize;
use serde_json::Value;
use super::{ApiError, NativeV2CliError, UiState, blocking, decode, encoded, workspace_changed};

pub(super) async fn list(State(state): State<UiState>) -> Result<Json<Value>, ApiError> {
    encoded(blocking(move || state.store.environments()).await?)
}

pub(super) async fn show(
    State(state): State<UiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let id = EnvironmentId::new(id).map_err(|error| invalid(error.to_string()))?;
    encoded(blocking(move || state.store.environment(&id)).await?)
}

pub(super) async fn save(
    State(state): State<UiState>,
    headers: HeaderMap,
    bytes: super::RequestBody,
) -> Result<Json<Value>, ApiError> {
    require_workspace(&headers, &state)?;
    let request: RuntimeEnvironmentSaveRequest = decode(bytes)?;
    request
        .definition
        .validate()
        .map_err(|error| invalid(error.to_string()))?;
    encoded(
        blocking(move || {
            state
                .store
                .save_environment(request, Some(&state.workspace.id))
        })
        .await?,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeleteRevision {
    expected_revision: EnvironmentRevision,
}

pub(super) async fn delete(
    State(state): State<UiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    bytes: super::RequestBody,
) -> Result<Json<Value>, ApiError> {
    require_workspace(&headers, &state)?;
    let id = EnvironmentId::new(id).map_err(|error| invalid(error.to_string()))?;
    let request: DeleteRevision = decode(bytes)?;
    encoded(
        blocking(move || {
            state.store.delete_environment(
                RuntimeEnvironmentDeleteRequest {
                    id,
                    expected_revision: request.expected_revision,
                },
                Some(&state.workspace.id),
            )
        })
        .await?,
    )
}

fn require_workspace(headers: &HeaderMap, state: &UiState) -> Result<(), ApiError> {
    let mut values = headers.get_all("x-zeroshot-workspace").iter();
    if values.next().and_then(|value| value.to_str().ok()) != Some(state.workspace.id.as_str())
        || values.next().is_some()
    {
        return Err(workspace_changed());
    }
    Ok(())
}

fn invalid(message: String) -> ApiError {
    ApiError {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        code: "invalid_environment",
        message,
    }
}

pub(super) fn local_error(error: NativeV2CliError) -> ApiError {
    let (status, code) = match &error {
        NativeV2CliError::EnvironmentMissing(_) => (StatusCode::NOT_FOUND, "environment_not_found"),
        NativeV2CliError::EnvironmentConflict => (StatusCode::CONFLICT, "environment_conflict"),
        NativeV2CliError::EnvironmentInUse => (StatusCode::CONFLICT, "environment_in_use"),
        NativeV2CliError::EnvironmentWorkspaceChanged => return workspace_changed(),
        _ => return ApiError::internal(error.to_string()),
    };
    ApiError {
        status,
        code,
        message: error.to_string(),
    }
}
