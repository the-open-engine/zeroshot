//! Host lifecycle and browser boundary for the shared workspace.
use std::future::IntoFuture;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use include_dir::{include_dir, Dir};
use openengine_cluster_protocol::{TargetRunHistoryDiscovery, TargetRunHistoryRoutes, RUN_HISTORY_KIND};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::{
    local_error, problem, router, runs::NativeRunHistory, LocalRunProfileStore, NativeV2CliError,
    RunHistoryTransport, UiState,
};

static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../ui/dist");
const DRAIN_TIMEOUT: Duration = Duration::from_secs(3);
pub(super) const TARGET_RUN_HISTORY_LIST_PATH: &str = "/native-v2/run-history";
pub(super) const TARGET_RUN_HISTORY_DETAIL_PATH: &str = "/native-v2/run-history/{id}";
pub(super) const TARGET_RUN_HISTORY_PAGE_PATH: &str = "/native-v2/run-history/{id}/page";

#[derive(Clone)]
pub(super) struct Shutdown(watch::Sender<bool>);

impl Default for Shutdown {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}

impl Shutdown {
    fn cancel(&self) {
        self.0.send_replace(true);
    }

    pub(super) async fn cancelled(&self) {
        let mut receiver = self.0.subscribe();
        let _ = receiver.wait_for(|stopped| *stopped).await;
    }
}

/// One application and observation lifetime. Hosts own listeners, authorization, and controllers.
#[derive(Clone)]
pub struct UiService {
    router: Router,
    shutdown: Shutdown,
    history_discovery: Option<TargetRunHistoryDiscovery>,
}

/// A configured target whose run history is read by the local UI server. Target coordinates and
/// hosted authority stay server-side; browsers continue to use only the local UI origin.
pub struct RunHistoryTarget {
    runs: NativeRunHistory,
}

impl RunHistoryTarget {
    /// Uses one server-side target transport. Browser requests never receive its authority state.
    pub fn remote(transport: impl RunHistoryTransport + 'static) -> Self {
        Self {
            runs: NativeRunHistory::transported(std::sync::Arc::new(transport)),
        }
    }
}

impl UiService {
    fn new(state: UiState, target_history: bool) -> Self {
        let history_discovery = target_history.then(|| TargetRunHistoryDiscovery {
            kind: RUN_HISTORY_KIND.to_owned(),
            base_url: state.origin.origin.clone(),
            route_templates: TargetRunHistoryRoutes {
                list: format!("{TARGET_RUN_HISTORY_LIST_PATH}{{?after}}"),
                detail: "/native-v2/run-history/{run_id}".to_owned(),
                page: "/native-v2/run-history/{run_id}/page{?after}".to_owned(),
            },
        });
        Self {
            shutdown: state.shutdown.clone(),
            router: router(state, target_history),
            history_discovery,
        }
    }

    /// Direct standalone target composition. The target host must reject this mount for private
    /// or hosted access; its broad target capabilities must never be exposed to a browser.
    pub fn for_target(
        storage: PathBuf,
        public_origin: &str,
        observations: crate::native_v2_observability::NativeV2Observability,
    ) -> Result<Self, NativeV2CliError> {
        if !storage.is_absolute() {
            return Err(NativeV2CliError::Local(
                "target UI storage must be absolute".into(),
            ));
        }
        Ok(Self::new(
            UiState::new(
                LocalRunProfileStore::new(storage.clone()),
                NativeRunHistory::target(storage, observations),
                public_origin,
                "target",
            )?,
            true,
        ))
    }

    pub(crate) fn run_history_discovery(&self) -> Option<TargetRunHistoryDiscovery> {
        self.history_discovery.clone()
    }

    /// Serves only UI routes on an already-routed target connection, including its keepalive
    /// requests. It cannot forward subsequent requests into the native target control plane.
    pub async fn serve_connection(&self, stream: TcpStream) -> io::Result<()> {
        let mut builder = hyper::server::conn::http1::Builder::new();
        builder
            .timer(TokioTimer::new())
            .header_read_timeout(Duration::from_secs(10))
            .max_buf_size(32 * 1024);
        let connection = builder.serve_connection(
            TokioIo::new(stream),
            TowerToHyperService::new(self.router.clone()),
        );
        tokio::pin!(connection);
        tokio::select! {
            result = &mut connection => return result.map_err(io::Error::other),
            () = self.shutdown.cancelled() => {}
        }
        connection.as_mut().graceful_shutdown();
        match tokio::time::timeout(DRAIN_TIMEOUT, connection).await {
            Ok(result) => result.map_err(io::Error::other),
            Err(_) => Ok(()), // Dropping this connection closes a stalled observer only.
        }
    }

    /// Closes observers. This service never owns or cancels a run.
    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }
}

/// Runs the local workspace in this process with local CLI profiles and run history.
pub async fn serve(listen: SocketAddr) -> Result<(), NativeV2CliError> {
    serve_with_target(listen, None).await
}

/// Runs the local workspace with profiles kept local and optional configured-target history.
pub async fn serve_with_target(
    listen: SocketAddr,
    target: Option<RunHistoryTarget>,
) -> Result<(), NativeV2CliError> {
    if !listen.ip().is_loopback() {
        return Err(NativeV2CliError::Local(
            "the local UI must listen on a loopback address".into(),
        ));
    }
    let listener = TcpListener::bind(listen).await.map_err(local_error)?;
    let address = listener.local_addr().map_err(local_error)?;
    let origin = format!("http://{address}");
    let service = UiService::new(
        UiState::new(
            LocalRunProfileStore::production()?,
            match target {
                Some(target) => target.runs,
                None => NativeRunHistory::production()?,
            },
            &origin,
            "local",
        )?,
        false,
    );
    let stopped = service.shutdown.clone();
    let server = axum::serve(listener, service.router.clone())
        .with_graceful_shutdown(async move { stopped.cancelled().await })
        .into_future();
    tokio::pin!(server);
    eprintln!("Zeroshot UI: {origin}/ui/");
    tokio::select! {
        result = &mut server => return result.map_err(local_error),
        result = shutdown_signal() => result.map_err(local_error)?,
    }
    service.shutdown();
    tokio::time::timeout(DRAIN_TIMEOUT, server)
        .await
        .map_err(|_| {
            local_error(io::Error::new(
                io::ErrorKind::TimedOut,
                "UI connections did not close",
            ))
        })?
        .map_err(local_error)
}

async fn shutdown_signal() -> io::Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await
}

#[derive(Clone)]
pub(super) struct BrowserOrigin {
    origin: String,
    authority: String,
}

impl BrowserOrigin {
    pub(super) fn new(value: &str) -> Result<Self, NativeV2CliError> {
        let url = parse_http_origin(value).map_err(|()| {
            NativeV2CliError::Local("UI public origin must be an HTTP(S) origin".into())
        })?;
        Ok(Self {
            origin: url.origin().ascii_serialization(),
            authority: url[url::Position::BeforeHost..url::Position::AfterPort].to_owned(),
        })
    }

    fn accepts(&self, headers: &HeaderMap) -> bool {
        let host = exact_header(headers, header::HOST.as_str());
        let origin = exact_header(headers, header::ORIGIN.as_str());
        let site = exact_header(headers, "sec-fetch-site");
        host == Ok(Some(self.authority.as_str()))
            && origin.is_ok_and(|value| value.is_none_or(|value| value == self.origin))
            && site.is_ok_and(|value| matches!(value, None | Some("same-origin" | "none")))
    }
}

pub(super) fn parse_http_origin(value: &str) -> Result<url::Url, ()> {
    if value
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return Err(());
    }
    let url = url::Url::parse(value).map_err(|_| ())?;
    valid_http_origin(&url).then_some(url).ok_or(())
}

fn valid_http_origin(url: &url::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
}

fn exact_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let value = values
        .next()
        .map(|value| value.to_str().map_err(|_| ()))
        .transpose()?;
    if values.next().is_some() {
        return Err(());
    }
    Ok(value)
}

pub(super) async fn browser_boundary(
    State(state): State<UiState>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    if !state.origin.accepts(headers) {
        return secured(problem(
            StatusCode::FORBIDDEN,
            "origin_rejected",
            "Open the UI using its configured public URL.",
        ));
    }
    if *state.shutdown.0.borrow() {
        return secured(problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "server_stopping",
            "The UI server is stopping.",
        ));
    }
    if request.method() == axum::http::Method::POST
        && exact_header(headers, header::CONTENT_TYPE.as_str())
            .ok()
            .flatten()
            .is_none_or(|value| {
                value
                    .split(';')
                    .next()
                    .is_none_or(|value| value.trim() != "application/json")
            })
    {
        return secured(problem(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "json_required",
            "Send an application/json request.",
        ));
    }
    secured(next.run(request).await)
}

fn secured(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("content-security-policy", HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self'; img-src 'self' data:; connect-src 'self'; worker-src 'self'; frame-ancestors 'self'; base-uri 'self'; form-action 'none'"));
    response
}

pub(super) async fn index() -> Response {
    file_response("index.html")
}

pub(super) async fn asset(Path(path): Path<String>) -> Response {
    file_response(&path)
}

fn file_response(path: &str) -> Response {
    let Some(file) = ASSETS.get_file(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mime = match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("woff2") => "font/woff2",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, mime)], file.contents()).into_response()
}

#[cfg(test)]
#[path = "server/tests.rs"]
mod tests;
