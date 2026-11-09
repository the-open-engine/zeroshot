//! Bounded HTTPS exchange. API paths and credentials never come from returned pagination links.
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;
use reqwest::{Client, Method, Url, redirect::Policy};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::OnceCell;
use url::Position;
use crate::source_code_provider::{SourceDeliveryCredential, SourceDeliveryError};
use super::wire::ServerMetadata;

const RESPONSE_LIMIT: usize = 1024 * 1024;
/// GitLab explains a rejection in a short JSON body; longer bodies keep only their prefix.
const ERROR_BODY_LIMIT: usize = 4 * 1024;
pub(super) const PAGE_LIMIT: u32 = 64;
const PAGE_SIZE: usize = 100;

#[derive(Clone)]
pub(super) struct Api {
    endpoint: Url,
    client: Client,
    metadata: Arc<OnceCell<ServerMetadata>>,
}

pub(super) struct ApiRequest<'a> {
    method: Method,
    segments: &'a [&'a str],
    query: Vec<(&'a str, String)>,
    body: Option<Value>,
}

impl<'a> ApiRequest<'a> {
    pub(super) fn get(segments: &'a [&'a str]) -> Self {
        Self {
            method: Method::GET,
            segments,
            query: Vec::new(),
            body: None,
        }
    }

    pub(super) fn write(segments: &'a [&'a str], method: Method, body: Value) -> Self {
        Self {
            method,
            segments,
            query: Vec::new(),
            body: Some(body),
        }
    }

    pub(super) fn query(mut self, key: &'a str, value: impl Into<String>) -> Self {
        self.query.push((key, value.into()));
        self
    }
}

impl Api {
    pub(super) fn new(endpoint: &str) -> Result<Self, SourceDeliveryError> {
        let endpoint = Url::parse(endpoint).map_err(|_| SourceDeliveryError::Rejected)?;
        if endpoint.scheme() != "https" {
            return Err(SourceDeliveryError::Rejected);
        }
        Self::build(endpoint)
    }

    fn build(endpoint: Url) -> Result<Self, SourceDeliveryError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(90))
            .connect_timeout(Duration::from_secs(10))
            .redirect(Policy::none())
            .build()
            .map_err(|_| SourceDeliveryError::Unavailable)?;
        Ok(Self {
            endpoint,
            client,
            metadata: Arc::new(OnceCell::new()),
        })
    }

    #[cfg(test)]
    pub(super) fn loopback(endpoint: &str) -> Result<Self, SourceDeliveryError> {
        let endpoint = Url::parse(endpoint).map_err(|_| SourceDeliveryError::Rejected)?;
        let local = endpoint
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|address| address.is_loopback());
        if endpoint.scheme() != "http" || !local {
            return Err(SourceDeliveryError::Rejected);
        }
        Self::build(endpoint)
    }

    pub(super) async fn server_metadata(
        &self,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<&ServerMetadata, SourceDeliveryError> {
        self.metadata
            .get_or_try_init(|| async {
                let value: ServerMetadata =
                    self.json(ApiRequest::get(&["version"]), credential).await?;
                require_version(&value.version)?;
                Ok(value)
            })
            .await
    }

    pub(super) async fn json<T: DeserializeOwned>(
        &self,
        request: ApiRequest<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<T, SourceDeliveryError> {
        let bytes = self.bytes(request, credential).await?;
        serde_json::from_slice(&bytes)
            .map_err(|_| SourceDeliveryError::api(None, "GitLab returned invalid JSON"))
    }

    pub(super) async fn pages<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Vec<T>, SourceDeliveryError> {
        let mut result = Vec::new();
        for page in 1..=PAGE_LIMIT {
            let values: Vec<T> = self
                .json(
                    ApiRequest::get(segments)
                        .query("per_page", PAGE_SIZE.to_string())
                        .query("page", page.to_string()),
                    credential,
                )
                .await?;
            let complete = values.len() < PAGE_SIZE;
            result.extend(values);
            if complete {
                return Ok(result);
            }
        }
        Err(SourceDeliveryError::api(
            None,
            "GitLab pagination exceeded its complete-read bound",
        ))
    }

    pub(super) async fn bytes(
        &self,
        request: ApiRequest<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Vec<u8>, SourceDeliveryError> {
        response_bytes(self.response(request, credential).await?, credential).await
    }

    /// Notes can be filtered after pagination, so item count cannot prove the final page.
    pub(super) async fn json_page<T: DeserializeOwned>(
        &self,
        request: ApiRequest<'_>,
        page: u32,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(Vec<T>, bool), SourceDeliveryError> {
        if page == 0 || page > PAGE_LIMIT {
            return Err(SourceDeliveryError::Rejected);
        }
        // Error statuses fail first, so a rejection without pagination headers keeps its status.
        let response = self.response(request, credential).await?;
        let has_more = notes_have_next_page(response.headers(), page)?;
        let bytes = response_bytes(response, credential).await?;
        let values: Vec<T> = serde_json::from_slice(&bytes)
            .map_err(|_| SourceDeliveryError::api(None, "GitLab returned invalid JSON"))?;
        if values.len() > PAGE_SIZE {
            return Err(SourceDeliveryError::api(
                None,
                "GitLab page exceeded its item bound",
            ));
        }
        Ok((values, has_more))
    }

    /// Returns only successful responses; a rejection keeps its status and GitLab's reason.
    async fn response(
        &self,
        request: ApiRequest<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<reqwest::Response, SourceDeliveryError> {
        let mut url = self.endpoint.clone();
        url.path_segments_mut()
            .map_err(|_| SourceDeliveryError::Rejected)?
            .pop_if_empty()
            .extend(request.segments.iter().copied());
        if !request.query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(request.query.iter().map(|(k, v)| (*k, v)));
        }
        let resource = format!("{} {}", request.method, &url[..Position::AfterPath]);
        let mut builder = self
            .client
            .request(request.method, url)
            .bearer_auth(credential.expose())
            .header("User-Agent", "zeroshot-gitlab-delivery")
            .header("Accept", "application/json");
        if let Some(body) = request.body {
            builder = builder.json(&body);
        }
        let response = builder.send().await.map_err(|error| {
            transport_failure(
                format!("GitLab API {resource} transport failed"),
                error,
                credential,
            )
        })?;
        if !response.status().is_success() {
            return Err(rejection(&resource, response, credential).await);
        }
        Ok(response)
    }
}

/// The status alone classifies the failure; the body only explains it.
async fn rejection(
    resource: &str,
    mut response: reqwest::Response,
    credential: SourceDeliveryCredential<'_>,
) -> SourceDeliveryError {
    let status = response.status().as_u16();
    let mut body = Vec::new();
    let truncated = loop {
        match response.chunk().await {
            Ok(Some(chunk)) if body.len() + chunk.len() <= ERROR_BODY_LIMIT => {
                body.extend_from_slice(&chunk);
            }
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk[..ERROR_BODY_LIMIT - body.len()]);
                break true;
            }
            Ok(None) => break false,
            Err(_) => break true,
        }
    };
    let mut diagnostic = format!("GitLab API {resource} returned HTTP {status}").into_bytes();
    if !body.is_empty() {
        diagnostic.extend_from_slice(b": ");
        diagnostic.extend_from_slice(&body);
    }
    SourceDeliveryError::api(
        Some(status),
        display_text(diagnostic, truncated, credential),
    )
}

fn transport_failure(
    context: String,
    error: reqwest::Error,
    credential: SourceDeliveryCredential<'_>,
) -> SourceDeliveryError {
    let error = error.without_url();
    let mut detail = context;
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(current) = cause {
        let _ = write!(detail, ": {current}");
        // Operating systems localize their messages; the portable kind keeps the cause readable.
        if let Some(kind) = current
            .downcast_ref::<std::io::Error>()
            .filter(|error| error.raw_os_error().is_some())
            .map(std::io::Error::kind)
        {
            let _ = write!(detail, " [{kind}]");
        }
        cause = current.source();
    }
    SourceDeliveryError::api(None, display_text(detail.into_bytes(), false, credential)).temporary()
}

/// Forge and transport text is redacted before truncation and kept on one control-free line.
fn display_text(
    bytes: Vec<u8>,
    truncated: bool,
    credential: SourceDeliveryCredential<'_>,
) -> String {
    // Trim raw secret prefixes before UTF8 replacement can change their bytes.
    let (text, truncated) =
        crate::native_v2_delivery::command::diagnostic_text(bytes, truncated, credential.expose());
    let encoded = credential_segment(credential.expose());
    let (text, truncated) =
        crate::native_v2_delivery::command::diagnostic_text(text.into_bytes(), truncated, &encoded);
    let mut text = text.replace(char::is_control, " ");
    if truncated {
        text.push_str(" [truncated]");
    }
    text
}

// Match the request encoder, including slash and percent escapes, without changing its URL.
fn credential_segment(token: &str) -> String {
    let mut url = Url::parse("https://redacted.invalid/").expect("static hierarchical URL");
    url.path_segments_mut()
        .expect("HTTP URL supports path segments")
        .push(token);
    url.path()[1..].to_owned()
}

async fn response_bytes(
    mut response: reqwest::Response,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Vec<u8>, SourceDeliveryError> {
    if response
        .content_length()
        .is_some_and(|length| length > RESPONSE_LIMIT as u64)
    {
        return Err(SourceDeliveryError::api(
            None,
            "GitLab response exceeded its byte bound",
        ));
    }
    let source = response.url()[..Position::AfterPath].to_owned();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        transport_failure(
            format!("GitLab API response from {source} transport failed"),
            error,
            credential,
        )
    })? {
        if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
            return Err(SourceDeliveryError::api(
                None,
                "GitLab response exceeded its byte bound",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn require_version(version: &str) -> Result<(), SourceDeliveryError> {
    let mut numbers = version.split('-').next().unwrap_or("").split('.');
    let parsed = (
        numbers.next().and_then(|v| v.parse::<u64>().ok()),
        numbers.next().and_then(|v| v.parse::<u64>().ok()),
        numbers.next().and_then(|v| v.parse::<u64>().ok()),
    );
    match parsed {
        (Some(major), Some(minor), Some(patch))
            if numbers.next().is_none() && (major, minor, patch) >= super::MINIMUM_VERSION =>
        {
            Ok(())
        }
        _ => Err(SourceDeliveryError::identity(
            "GitLab server 19.5.1 or newer is required for delivery",
        )),
    }
}

fn invalid_pagination() -> SourceDeliveryError {
    SourceDeliveryError::api(
        None,
        "GitLab notes pagination metadata is missing, ambiguous, or outside its consecutive page bound",
    )
}

fn notes_have_next_page(
    headers: &reqwest::header::HeaderMap,
    page: u32,
) -> Result<bool, SourceDeliveryError> {
    if headers.get_all("x-next-page").iter().count() != 1 {
        return Err(invalid_pagination());
    }
    let value = headers
        .get("x-next-page")
        .ok_or_else(invalid_pagination)?
        .to_str()
        .map_err(|_| invalid_pagination())?;
    if value.is_empty() {
        return Ok(false);
    }
    let next = value.parse::<u32>().map_err(|_| invalid_pagination())?;
    (next == page.saturating_add(1) && next <= PAGE_LIMIT && value == next.to_string())
        .then_some(true)
        .ok_or_else(invalid_pagination)
}
