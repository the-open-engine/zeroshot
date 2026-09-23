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
    RunProfileSelector, RunProfileSetRequest, RuntimePlan,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::workspace;
use crate::native_v2_cli::{
    profile_revision, LocalRunProfileStore, NativeV2CliError, ProfileSaveConflict,
};

mod runs;
mod server;
pub use server::{serve, UiService};

const MAX_BODY: usize = 2 * 1024 * 1024;

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

fn router(state: UiState) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::temporary("/ui/") }))
        .route("/ui", get(|| async { Redirect::temporary("/ui/") }))
        .route("/ui/", get(server::index))
        .route("/ui/api/bootstrap", get(bootstrap))
        .route("/ui/api/profiles", get(list).post(save))
        .route("/ui/api/profiles/{name}", get(show))
        .route("/ui/api/runs", get(runs::list))
        .route("/ui/api/runs/{id}", get(runs::show))
        .route("/ui/api/runs/{id}/history", get(runs::history))
        .route("/ui/api/runs/{id}/events", get(runs::events))
        .route("/ui/api/validate", post(validate))
        .route("/ui/api/authoring", post(author))
        .route("/ui/api/data", post(data_author))
        .route("/ui/{*asset}", get(server::asset))
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
    runtime: RuntimePlan,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SaveRequest {
    name: RunProfileName,
    graph: GraphSpec,
    runtime: RuntimePlan,
    expected_revision: Option<String>,
}

async fn validate(
    bytes: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<Json<Value>, ApiError> {
    let document: ProfileDocument = decode(bytes)?;
    admit(&document.graph, &document.runtime).await?;
    Ok(Json(json!({"valid":true})))
}
async fn author(
    bytes: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(workspace::author(decode(bytes)?)?))
}
async fn data_author(
    bytes: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(workspace::data(decode(bytes)?)?))
}
async fn save(
    State(state): State<UiState>,
    headers: HeaderMap,
    bytes: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<Json<Value>, ApiError> {
    let mut workspace = headers.get_all("x-zeroshot-workspace").iter();
    if workspace.next().and_then(|value| value.to_str().ok()) != Some(state.workspace.id.as_str())
        || workspace.next().is_some()
    {
        return Err(workspace_changed());
    }
    let request: SaveRequest = decode(bytes)?;
    admit(&request.graph, &request.runtime).await?;
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
    workspace::validate_profile(graph, runtime)
        .await
        .map_err(Into::into)
}
fn decode<T: serde::de::DeserializeOwned>(
    bytes: Result<Bytes, axum::extract::rejection::BytesRejection>,
) -> Result<T, ApiError> {
    let bytes = bytes
        .map_err(|_| ApiError::invalid("The profile exceeds the 2 MiB request limit.".into()))?;
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
        .map_err(|e| ApiError::internal(e.to_string()))
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
mod tests {
    use super::*;
    use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
    use openengine_cluster_testkit::assertions::AssertValue;
    struct Server {
        url: String,
        task: tokio::task::JoinHandle<()>,
        root: std::path::PathBuf,
        workspace: String,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    async fn server() -> Server {
        let root = std::env::temp_dir().join(format!("zeroshot-ui-test-{}", uuid::Uuid::now_v7()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .assert_value();
        let authority = listener.local_addr().assert_value().to_string();
        let state = UiState::new(
            LocalRunProfileStore::new(root.clone()),
            runs::NativeRunHistory::new(root.join("state")),
            &format!("http://{authority}"),
            "local",
        )
        .assert_value();
        let workspace = state.workspace.id.clone();
        let app = router(state);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.assert_value();
        });
        Server {
            url: format!("http://{authority}"),
            task,
            root,
            workspace,
        }
    }
    fn profile_request() -> Value {
        json!({"name":"browser-test","graph":BuiltinGraphTemplate::SingleWorker.materialize(TemplateDelivery::None).assert_value(),"runtime":{"harness":"codex","provider":"openai","size":"small","nodes":{"worker":{"kind":"agent","model":"opaque-model"}}},"expectedRevision":null})
    }
    #[tokio::test]
    async fn browser_save_is_cli_visible_and_conflicts_are_atomic() {
        let server = server().await;
        let client = reqwest::Client::new();
        let url = format!("{}/ui/api/profiles", server.url);
        let request = profile_request();
        let first = client
            .post(&url)
            .header("x-zeroshot-workspace", &server.workspace)
            .json(&request)
            .send()
            .await
            .assert_value();
        assert_eq!(first.status(), StatusCode::OK);
        let saved: Value = first.json().await.assert_value();
        let store = LocalRunProfileStore::new(server.root.clone());
        let shown = store
            .show(RunProfileSelector {
                name: RunProfileName::new("browser-test").assert_value(),
                scope: RunProfileScope::User,
            })
            .assert_value();
        assert_eq!(
            serde_json::to_value(&shown).assert_value(),
            saved["profile"]
        );
        assert_eq!(
            client
                .post(&url)
                .header("x-zeroshot-workspace", &server.workspace)
                .json(&request)
                .send()
                .await
                .assert_value()
                .status(),
            StatusCode::CONFLICT
        );
        let mut update = request;
        update["expectedRevision"] = saved["revision"].clone();
        update["runtime"]["nodes"]["worker"]["model"] = json!("updated-model");
        let (a, b) = tokio::join!(
            client
                .post(&url)
                .header("x-zeroshot-workspace", &server.workspace)
                .json(&update)
                .send(),
            client
                .post(&url)
                .header("x-zeroshot-workspace", &server.workspace)
                .json(&update)
                .send()
        );
        let statuses = [a.assert_value().status(), b.assert_value().status()];
        assert!(statuses.contains(&StatusCode::OK));
        assert!(statuses.contains(&StatusCode::CONFLICT));
    }
    #[tokio::test]
    async fn invalid_profile_never_enters_store() {
        let server = server().await;
        let client = reqwest::Client::new();
        let mut request = profile_request();
        request["runtime"]["nodes"] = json!({});
        let response = client
            .post(format!("{}/ui/api/profiles", server.url))
            .header("x-zeroshot-workspace", &server.workspace)
            .json(&request)
            .send()
            .await
            .assert_value();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let problem: Value = response.json().await.assert_value();
        assert_eq!(problem["code"], "invalid_profile");
        assert!(!server.root.join("profiles.json").exists());
    }
    #[tokio::test]
    async fn profile_writes_require_one_matching_workspace_identity() {
        let server = server().await;
        let client = reqwest::Client::new();
        let url = format!("{}/ui/api/profiles", server.url);
        for identities in [
            vec![],
            vec!["another-workspace"],
            vec![server.workspace.as_str(), server.workspace.as_str()],
        ] {
            let mut request = client.post(&url).json(&profile_request());
            for identity in identities {
                request = request.header("x-zeroshot-workspace", identity);
            }
            let response = request.send().await.assert_value();
            assert_eq!(response.status(), StatusCode::CONFLICT);
            let problem: Value = response.json().await.assert_value();
            assert_eq!(problem["code"], "workspace_changed");
            assert!(
                problem["message"]
                    .as_str()
                    .assert_value()
                    .contains("Reload")
            );
            assert!(!server.root.join("profiles.json").exists());
        }
    }
    #[tokio::test]
    async fn a_replaced_store_rejects_an_old_tabs_new_profile_without_a_revision() {
        let server = server().await;
        std::fs::remove_dir_all(&server.root).assert_value();
        let replacement = LocalRunProfileStore::new(server.root.clone());
        let new_identity = replacement.workspace_id().assert_value();
        assert_ne!(new_identity, server.workspace);
        let response = reqwest::Client::new()
            .post(format!("{}/ui/api/profiles", server.url))
            .header("x-zeroshot-workspace", &server.workspace)
            .json(&profile_request())
            .send()
            .await
            .assert_value();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let problem: Value = response.json().await.assert_value();
        assert_eq!(problem["code"], "workspace_changed");
        assert!(!server.root.join("profiles.json").exists());
    }
    #[tokio::test]
    async fn authoring_returns_only_a_draft_and_uses_the_browser_boundary() {
        let server = server().await;
        let client = reqwest::Client::new();
        let url = format!("{}/ui/api/authoring", server.url);
        let mut request = profile_request();
        request.as_object_mut().assert_value().remove("name");
        request
            .as_object_mut()
            .assert_value()
            .remove("expectedRevision");
        request["runtime"] =
            json!({"harness":"","provider":"","nodes":{"worker":{"kind":"agent","model":""}}});
        request["graph"]["root"] = json!({"kind":"seq","name":"run","state":{"kind":"record","fields":{}},"children":[{"kind":"fail","name":"failed","reason":"old_reason"}],"promotedStatePaths":[]});
        request["action"] =
            json!({"kind":"failure_reason","terminal":"failed","reason":"budget_exhausted"});
        let response = client.post(&url).json(&request).send().await.assert_value();
        assert_eq!(response.status(), StatusCode::OK);
        let result: Value = response.json().await.assert_value();
        assert_eq!(
            result["graph"]["root"]["children"][0]["reason"],
            "budget_exhausted"
        );
        assert_eq!(result["runtime"], request["runtime"]);
        assert!(result.get("profile").is_none());
        assert!(!server.root.join("profiles.json").exists());
        for reason in ["unhandled", "runtime_failed", "runtime_lost"] {
            request["action"]["reason"] = json!(reason);
            assert_eq!(
                client
                    .post(&url)
                    .json(&request)
                    .send()
                    .await
                    .assert_value()
                    .status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
        assert_eq!(
            client
                .post(&url)
                .header("origin", "https://untrusted.example")
                .json(&request)
                .send()
                .await
                .assert_value()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert!(!server.root.join("profiles.json").exists());
    }
    #[tokio::test]
    async fn local_browser_boundary_rejects_foreign_origins_and_dns_rebinding() {
        let server = server().await;
        let client = reqwest::Client::new();
        let url = format!("{}/ui/api/profiles", server.url);
        for header in [
            ("origin", "https://untrusted.example"),
            ("host", "untrusted.example"),
            ("sec-fetch-site", "cross-site"),
        ] {
            let response = client
                .get(&url)
                .header(header.0, header.1)
                .send()
                .await
                .assert_value();
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        let response = client.post(&url).body("{}").send().await.assert_value();
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    #[tokio::test]
    async fn static_assets_and_bootstrap_are_native_and_self_contained() {
        let server = server().await;
        let client = reqwest::Client::new();
        let page = client
            .get(format!("{}/ui/", server.url))
            .send()
            .await
            .assert_value();
        assert_eq!(page.status(), StatusCode::OK);
        assert!(
            page.headers()["content-security-policy"]
                .to_str()
                .assert_value()
                .contains("worker-src 'self'")
        );
        assert!(
            page.text()
                .await
                .assert_value()
                .contains("Profiles · Zeroshot")
        );
        let bootstrap: Value = client
            .get(format!("{}/ui/api/bootstrap", server.url))
            .send()
            .await
            .assert_value()
            .json()
            .await
            .assert_value();
        assert_eq!(bootstrap["templates"].as_array().assert_value().len(), 7);
        assert_eq!(bootstrap["workers"].as_array().assert_value().len(), 4);
        assert!(bootstrap["runtimeSchema"]["$defs"].is_object());
    }
}
