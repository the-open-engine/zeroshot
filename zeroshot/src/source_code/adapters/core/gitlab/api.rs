//! Bounded HTTPS exchange. API paths and credentials never come from returned pagination links.
use std::sync::Arc;
use std::time::Duration;
use reqwest::{Client, Method, Url, redirect::Policy};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::OnceCell;
use crate::source_code_provider::{SourceDeliveryCredential, SourceDeliveryError};

const RESPONSE_LIMIT: usize = 1024 * 1024;
pub(super) const PAGE_LIMIT: u32 = 64;
const PAGE_SIZE: usize = 100;

#[derive(Clone)]
pub(super) struct Api {
    endpoint: Url,
    client: Client,
    version: Arc<OnceCell<()>>,
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
            version: Arc::new(OnceCell::new()),
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

    pub(super) async fn ensure_version(
        &self,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        self.version
            .get_or_try_init(|| async {
                let value: Value = self.json(ApiRequest::get(&["version"]), credential).await?;
                let version = value
                    .get("version")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        SourceDeliveryError::identity("GitLab did not report a server version")
                    })?;
                require_version(version)
            })
            .await
            .map(|_| ())
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
        response_bytes(self.response(request, credential).await?).await
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
        let response = self.response(request, credential).await?;
        // Preserve HTTP authentication/status failures even when error responses lack pagination.
        let has_more =
            response.status().is_success() && notes_have_next_page(response.headers(), page)?;
        let bytes = response_bytes(response).await?;
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
        let mut builder = self
            .client
            .request(request.method, url)
            .bearer_auth(credential.expose())
            .header("User-Agent", "zeroshot-gitlab-delivery")
            .header("Accept", "application/json");
        if let Some(body) = request.body {
            builder = builder.json(&body);
        }
        let response = builder.send().await.map_err(|_| {
            SourceDeliveryError::api(None, "GitLab API transport failed").temporary()
        })?;
        Ok(response)
    }
}

async fn response_bytes(mut response: reqwest::Response) -> Result<Vec<u8>, SourceDeliveryError> {
    let status = response.status();
    if !status.is_success() {
        return Err(SourceDeliveryError::api(
            Some(status.as_u16()),
            format!("GitLab API returned HTTP {}", status.as_u16()),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > RESPONSE_LIMIT as u64)
    {
        return Err(SourceDeliveryError::api(
            None,
            "GitLab response exceeded its byte bound",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        SourceDeliveryError::api(None, "GitLab response transport failed").temporary()
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
