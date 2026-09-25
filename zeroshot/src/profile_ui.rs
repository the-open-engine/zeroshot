//! Shared browser workspace services. Local and target hosts supply their storage and origin;
//! graph admission stays in Rust. Observation never owns a controller or starts a run.
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use openengine_cluster_protocol::{
    GraphSpec, RunProfile, RunProfileListRequest, RunProfileName, RunProfileScope,
    RunProfileSelector, RunProfileSetRequest, ProfileRuntimePlan, RuntimePlan,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::workspace;
use crate::native_v2_cli::{
    profile_revision, LocalRunProfileStore, NativeV2CliError, ProfileSaveConflict,
};

mod environments;
mod run_history_transport;
mod runs;
mod server;
pub use run_history_transport::{
    RunHistoryRequest, RunHistoryResponse, RunHistoryTransport, RunHistoryTransportError,
};
pub use server::{serve, serve_with_target, RunHistoryTarget, UiService};

const MAX_BODY: usize = 2 * 1024 * 1024;
const MAX_ENVIRONMENT_BODY: usize = 4 * 1024 * 1024;
type RequestBody = Result<Bytes, axum::extract::rejection::BytesRejection>;

#[derive(Clone)]
struct UiState {
    store: LocalRunProfileStore,
    runs: runs::NativeRunHistory,
    origin: server::BrowserOrigin,
    shutdown: server::Shutdown,
    workspace: UiWorkspace,
}

#[derive(Clone, Serialize)]
struct UiWorkspace {
    kind: &'static str,
    id: String,
}

impl UiState {
    fn new(
        store: LocalRunProfileStore,
        runs: runs::NativeRunHistory,
        origin: &str,
        kind: &'static str,
    ) -> Result<Self, NativeV2CliError> {
        let origin = server::BrowserOrigin::new(origin)?;
        let id = store.workspace_id()?;
        Ok(Self {
            store,
            runs,
            origin,
            shutdown: server::Shutdown::default(),
            workspace: UiWorkspace { kind, id },
        })
    }
}

fn router(state: UiState, target_history: bool) -> Router {
    let router = Router::new()
        .route("/", get(|| async { Redirect::temporary("/ui/") }))
        .route("/ui", get(|| async { Redirect::temporary("/ui/") }))
        .route("/ui/", get(server::index))
        .route("/ui/api/bootstrap", get(bootstrap))
        .route("/ui/api/profiles", get(list).post(save))
        .route("/ui/api/profiles/{name}", get(show))
        .route(
            "/ui/api/environments",
            get(environments::list)
                .post(environments::save)
                .layer(DefaultBodyLimit::max(MAX_ENVIRONMENT_BODY)),
        )
        .route(
            "/ui/api/environments/{id}",
            get(environments::show).delete(environments::delete),
        )
        .route("/ui/api/runs", get(runs::list))
        .route("/ui/api/runs/{id}", get(runs::show))
        .route("/ui/api/runs/{id}/history", get(runs::history))
        .route("/ui/api/runs/{id}/events", get(runs::events))
        .route("/ui/api/validate", post(validate))
        .route("/ui/api/authoring", post(author))
        .route("/ui/api/data", post(data_author))
        .route("/ui/{*asset}", get(server::asset));
    let router = if target_history {
        router
            .route(server::TARGET_RUN_HISTORY_LIST_PATH, get(runs::list))
            .route(server::TARGET_RUN_HISTORY_DETAIL_PATH, get(runs::show))
            .route(server::TARGET_RUN_HISTORY_PAGE_PATH, get(runs::history))
    } else {
        router
    };
    router
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            server::browser_boundary,
        ))
        .with_state(state)
}

async fn bootstrap(State(state): State<UiState>) -> Result<Json<Value>, ApiError> {
    let mut catalog = workspace::catalog()?;
    catalog["workspace"] = serde_json::to_value(state.workspace)
        .map_err(|error| ApiError::internal(error.to_string()))?;
    Ok(Json(catalog))
}
async fn list(State(state): State<UiState>) -> Result<Json<Value>, ApiError> {
    blocking(move || {
        state.store.list(RunProfileListRequest {
            scope: RunProfileScope::User,
        })
    })
    .await
    .and_then(encoded)
}
async fn show(
    State(state): State<UiState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let name = RunProfileName::new(name).map_err(|e| ApiError::invalid(e.to_string()))?;
    let profile = blocking(move || {
        state.store.show(RunProfileSelector {
            name,
            scope: RunProfileScope::User,
        })
    })
    .await?;
    envelope(profile)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProfileDocument {
    graph: GraphSpec,
    runtime: ProfileRuntimePlan,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SaveRequest {
    name: RunProfileName,
    graph: GraphSpec,
    runtime: ProfileRuntimePlan,
    expected_revision: Option<String>,
}

async fn validate(
    State(state): State<UiState>,
    bytes: RequestBody,
) -> Result<Json<Value>, ApiError> {
    let document: ProfileDocument = decode(bytes)?;
    let runtime = blocking(move || state.store.resolve_runtime(&document.runtime)).await?;
    admit(&document.graph, &runtime).await?;
    Ok(Json(json!({"valid":true})))
}
async fn author(bytes: RequestBody) -> Result<Json<Value>, ApiError> {
    Ok(Json(workspace::author(decode(bytes)?)?))
}
async fn data_author(bytes: RequestBody) -> Result<Json<Value>, ApiError> {
    Ok(Json(workspace::data(decode(bytes)?)?))
}
async fn save(
    State(state): State<UiState>,
    headers: HeaderMap,
    bytes: RequestBody,
) -> Result<Json<Value>, ApiError> {
    let mut workspace = headers.get_all("x-zeroshot-workspace").iter();
    if workspace.next().and_then(|value| value.to_str().ok()) != Some(state.workspace.id.as_str())
        || workspace.next().is_some()
    {
        return Err(workspace_changed());
    }
    let request: SaveRequest = decode(bytes)?;
    let store = state.store.clone();
    let runtime = request.runtime.clone();
    let resolved = blocking(move || store.resolve_runtime(&runtime)).await?;
    admit(&request.graph, &resolved).await?;
    let result = blocking(move || {
        state.store.set_checked(
            RunProfileSetRequest {
                name: request.name,
                scope: RunProfileScope::User,
                graph: request.graph,
                runtime: request.runtime,
                set_default: false,
            },
            request.expected_revision.as_deref(),
            &state.workspace.id,
        )
    })
    .await?;
    match result {
        Ok(result) => envelope(result.profile),
        Err(ProfileSaveConflict::Workspace) => Err(workspace_changed()),
        Err(ProfileSaveConflict::Revision) => Err(ApiError { status: StatusCode::CONFLICT, code:"profile_conflict", message:"This profile changed elsewhere, or the name is already taken. Reload it or save a copy with another name.".into() }),
    }
}
fn workspace_changed() -> ApiError {
    ApiError {
        status: StatusCode::CONFLICT,
        code: "workspace_changed",
        message: "This workspace changed. Reload before saving. Your draft is unchanged.".into(),
    }
}
async fn admit(graph: &GraphSpec, runtime: &RuntimePlan) -> Result<(), ApiError> {
    crate::native_v2_admission::NativeV2Admission
        .validate_profile(
            graph,
            runtime,
            crate::native_v2_admission::DeliveryPolicy::Optional,
        )
        .await
        .map_err(|error| ApiError::invalid(error.to_string()))
}
fn decode<T: serde::de::DeserializeOwned>(bytes: RequestBody) -> Result<T, ApiError> {
    let bytes = bytes
        .map_err(|_| ApiError::invalid("The request exceeds the endpoint's body limit.".into()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::invalid(format!("Invalid profile JSON: {e}")))
}
fn envelope(profile: RunProfile) -> Result<Json<Value>, ApiError> {
    let revision = profile_revision(&profile).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({"profile":profile,"revision":revision})))
}
fn encoded<T: serde::Serialize>(value: T) -> Result<Json<Value>, ApiError> {
    serde_json::to_value(value)
        .map(Json)
        .map_err(|e| ApiError::internal(e.to_string()))
}
async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T, NativeV2CliError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map_err(environments::local_error)
}
fn local_error(error: std::io::Error) -> NativeV2CliError {
    NativeV2CliError::Local(error.to_string())
}
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}
impl ApiError {
    fn invalid(message: String) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "invalid_profile",
            message,
        }
    }
    fn internal(message: String) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "profile_store_error",
            message,
        }
    }
}
impl From<workspace::WorkspaceError> for ApiError {
    fn from(error: workspace::WorkspaceError) -> Self {
        Self {
            status: StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            code: error.code,
            message: error.message,
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        problem(self.status, self.code, &self.message)
    }
}
fn problem(status: StatusCode, code: &str, message: &str) -> Response {
    (status, Json(json!({"code":code,"message":message}))).into_response()
}

#[cfg(test)]
#[path = "profile_ui/tests.rs"]
mod tests;
