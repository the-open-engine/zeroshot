//! Bounded HTTPS exchange with a Forgejo instance. Every request is built from the admitted
//! instance, so a server-supplied pagination link is never followed. The credential and network
//! values returned by `diagnostic_secrets` are the single set used for all Forgejo diagnostic text,
//! and never enter a URL, a printed diagnostic, a log line or a `Debug` value.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, Method, Url, header::HeaderMap, header::HeaderValue, redirect::Policy};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::OnceCell;
use url::Position;

use crate::native_v2_delivery::{command, git_auth};
use crate::source_code_provider::adapters::core::ci_diagnostics;
use crate::source_code_provider::adapters::core::network::ForgeNetwork;
use crate::source_code_provider::{SourceDeliveryCredential, SourceDeliveryError, SourceGitCredential};

use super::wire::ServerVersion;

pub(super) const RESPONSE_LIMIT: usize = 1024 * 1024;
/// A Forgejo rejection is a short message; a longer body keeps only its prefix.
pub(super) const ERROR_BODY_LIMIT: usize = 4 * 1024;
pub(super) const PAGE_LIMIT: u32 = 64;
pub(super) const PAGE_SIZE: usize = 50;
pub(super) const MINIMUM_VERSION: (u64, u64, u64) = (16, 0, 0);

#[derive(Clone)]
pub(super) struct Api {
    endpoint: Url,
    client: Client,
    version: Arc<OnceCell<String>>,
    network: ForgeNetwork,
}

impl std::fmt::Debug for Api {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Api([REDACTED])")
    }
}

#[derive(Clone)]
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
    pub(super) fn new(base_url: &str) -> Result<Self, SourceDeliveryError> {
        let endpoint = Url::parse(base_url)
            .map_err(|_| validation_error("base URL is not a valid absolute URL"))?;
        if endpoint.scheme() != "https" {
            return Err(validation_error("base URL must use https"));
        }
        Self::build(endpoint, None)
    }

    /// The admitted instance, including its installation path prefix.
    #[cfg(test)]
    pub(super) fn base(&self) -> &Url {
        &self.endpoint
    }

    pub(super) fn with_network(self, network: ForgeNetwork) -> Result<Self, SourceDeliveryError> {
        Self::build(self.endpoint, Some(network))
    }

    #[cfg(test)]
    pub(super) fn loopback(base_url: &str) -> Result<Self, SourceDeliveryError> {
        let endpoint = Url::parse(base_url)
            .map_err(|_| validation_error("test base URL is not a valid absolute URL"))?;
        if endpoint.scheme() != "http" || !loopback_host(endpoint.host_str().unwrap_or_default()) {
            return Err(validation_error(
                "test base URL must be http on a loopback address",
            ));
        }
        Self::build(endpoint, None)
    }

    fn build(endpoint: Url, network: Option<ForgeNetwork>) -> Result<Self, SourceDeliveryError> {
        let builder = Client::builder()
            .timeout(Duration::from_secs(90))
            .connect_timeout(Duration::from_secs(10))
            .redirect(Policy::none());
        let builder = match &network {
            Some(network) => network.client(builder)?,
            None => builder,
        };
        Ok(Self {
            endpoint,
            client: builder
                .build()
                .map_err(|_| validation_error("HTTP client could not be built"))?,
            version: Arc::new(OnceCell::new()),
            network: network.unwrap_or_default(),
        })
    }

    pub(super) async fn require_supported(
        &self,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        self.version
            .get_or_try_init(|| async {
                let observed: ServerVersion =
                    self.json(ApiRequest::get(&["version"]), credential).await?;
                require_version(&observed.version)?;
                Ok(observed.version)
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
        self.parse_json(&bytes, credential)
    }

    /// A missing resource is an answer, not a failure; every other status keeps its reason.
    pub(super) async fn optional_json<T: DeserializeOwned>(
        &self,
        request: ApiRequest<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Option<T>, SourceDeliveryError> {
        let response = match self.response(request, credential).await {
            Ok(response) => response,
            Err(error) if error.api_status() == Some(404) => return Ok(None),
            Err(error) => return Err(error),
        };
        let bytes = self
            .response_bytes(response, credential, RESPONSE_LIMIT)
            .await?;
        self.parse_json(&bytes, credential).map(Some)
    }

    /// A job log is plain text and can be far larger than a JSON response, so it is read under the
    /// shared CI log byte bound and kept as its redacted tail.
    pub(super) async fn job_log(
        &self,
        segments: &[&str],
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<ci_diagnostics::JobLog, SourceDeliveryError> {
        let bytes = self
            .response_bytes(
                self.response(ApiRequest::get(segments), credential).await?,
                credential,
                ci_diagnostics::MAX_JOB_LOG_BYTES,
            )
            .await?;
        let value = strip_log_timestamps(&String::from_utf8_lossy(&bytes));
        Ok(ci_diagnostics::job_log_tail(
            &value,
            value.len(),
            credential,
            &self.diagnostic_secrets(credential),
        ))
    }

    /// Pages come from the caller's own consecutive page numbers and the reported total, never
    /// from a link the server returned.
    pub(super) async fn pages<T: DeserializeOwned>(
        &self,
        request: ApiRequest<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Vec<T>, SourceDeliveryError> {
        let mut collected = Vec::new();
        for page in 1..=PAGE_LIMIT {
            let response = self
                .response(
                    request
                        .clone()
                        .query("limit", PAGE_SIZE.to_string())
                        .query("page", page.to_string()),
                    credential,
                )
                .await?;
            let total = self.total_count(response.headers(), credential)?;
            let bytes = self
                .response_bytes(response, credential, RESPONSE_LIMIT)
                .await?;
            let values: Vec<T> = self.parse_json(&bytes, credential)?;
            let count = values.len();
            if count > PAGE_SIZE {
                return Err(self.api_error("Forgejo page exceeded its item bound", credential));
            }
            collected.extend(values);
            if collected.len() > total {
                return Err(self.api_error(
                    format!(
                        "Forgejo pagination returned more items than its reported total of {total}"
                    ),
                    credential,
                ));
            }
            if collected.len() == total {
                return Ok(collected);
            }
            if count == 0 {
                // A shortened list could hide an existing pull request, so an empty page is only
                // the end of the read when the reported total is already collected.
                return Err(self.api_error(
                    format!(
                        "Forgejo pagination returned an empty page before the reported total of {total}"
                    ),
                    credential,
                ));
            }
        }
        Err(self.api_error(
            format!("Forgejo pagination exceeded its consecutive page bound of {PAGE_LIMIT}"),
            credential,
        ))
    }

    pub(super) async fn empty(
        &self,
        request: ApiRequest<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        self.bytes(request, credential).await.map(|_| ())
    }

    /// Returns only successful responses; a rejection keeps its status and Forgejo's reason.
    async fn response(
        &self,
        request: ApiRequest<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<reqwest::Response, SourceDeliveryError> {
        let mut url = self.endpoint.clone();
        url.path_segments_mut()
            .map_err(|_| SourceDeliveryError::Rejected)?
            .pop_if_empty()
            .push("api")
            .push("v1")
            .extend(request.segments.iter().copied());
        if !request.query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(request.query.iter().map(|(key, value)| (*key, value)));
        }
        let resource = format!("{} {}", request.method, &url[..Position::AfterPath]);
        let mut authorization = HeaderValue::from_str(&format!("token {}", credential.expose()))
            .map_err(|_| SourceDeliveryError::Rejected)?;
        authorization.set_sensitive(true);
        let mut builder = self
            .client
            .request(request.method, url)
            .header("authorization", authorization)
            .header("accept", "application/json");
        if let Some(body) = request.body {
            builder = builder.json(&body);
        }
        let response = builder.send().await.map_err(|error| {
            self.transport_failure(
                format!("Forgejo API {resource} transport failed"),
                error,
                credential,
            )
        })?;
        if !response.status().is_success() {
            return Err(self.rejection(&resource, response, credential).await);
        }
        Ok(response)
    }

    async fn rejection(
        &self,
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
        let mut diagnostic = format!("Forgejo API {resource} returned HTTP {status}").into_bytes();
        if !body.is_empty() {
            diagnostic.extend_from_slice(b": ");
            diagnostic.extend_from_slice(&body);
        }
        SourceDeliveryError::api(
            Some(status),
            self.display_text(diagnostic, truncated, credential),
        )
    }

    fn transport_failure(
        &self,
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
        SourceDeliveryError::api(
            None,
            self.display_text(detail.into_bytes(), false, credential),
        )
        .temporary()
    }

    async fn response_bytes(
        &self,
        mut response: reqwest::Response,
        credential: SourceDeliveryCredential<'_>,
        maximum_bytes: usize,
    ) -> Result<Vec<u8>, SourceDeliveryError> {
        if response
            .content_length()
            .is_some_and(|length| length > maximum_bytes as u64)
        {
            return Err(self.api_error("Forgejo response exceeded its byte bound", credential));
        }
        let source = response.url()[..Position::AfterPath].to_owned();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            self.transport_failure(
                format!("Forgejo API response from {source} transport failed"),
                error,
                credential,
            )
        })? {
            if bytes.len().saturating_add(chunk.len()) > maximum_bytes {
                return Err(self.api_error("Forgejo response exceeded its byte bound", credential));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn bytes(
        &self,
        request: ApiRequest<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Vec<u8>, SourceDeliveryError> {
        self.response_bytes(
            self.response(request, credential).await?,
            credential,
            RESPONSE_LIMIT,
        )
        .await
    }

    fn parse_json<T: DeserializeOwned>(
        &self,
        bytes: &[u8],
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<T, SourceDeliveryError> {
        serde_json::from_slice(bytes)
            .map_err(|_| self.api_error("Forgejo returned invalid JSON", credential))
    }

    /// Exactly one numeric total proves how many items the caller may still collect.
    fn total_count(
        &self,
        headers: &HeaderMap,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<usize, SourceDeliveryError> {
        let values = headers.get_all("x-total-count");
        if values.iter().count() != 1 {
            return Err(self.pagination_error(credential));
        }
        let value = values
            .iter()
            .next()
            .ok_or_else(|| self.pagination_error(credential))?
            .to_str()
            .map_err(|_| self.pagination_error(credential))?;
        let total = value
            .parse::<usize>()
            .map_err(|_| self.pagination_error(credential))?;
        (value == total.to_string())
            .then_some(total)
            .ok_or_else(|| self.pagination_error(credential))
    }

    fn api_error(
        &self,
        message: impl Into<String>,
        credential: SourceDeliveryCredential<'_>,
    ) -> SourceDeliveryError {
        SourceDeliveryError::api(
            None,
            self.display_text(message.into().into_bytes(), false, credential),
        )
    }

    fn pagination_error(&self, credential: SourceDeliveryCredential<'_>) -> SourceDeliveryError {
        self.api_error(
            "Forgejo pagination metadata is missing, ambiguous, or outside its consecutive page bound"
                .to_owned(),
            credential,
        )
    }

    /// Every value a printable diagnostic redacts: the stored login, its encoded forms, and the
    /// captured network proxy and certificate-trust values.
    pub(super) fn diagnostic_secrets(
        &self,
        credential: SourceDeliveryCredential<'_>,
    ) -> Vec<String> {
        let mut values = vec![credential.expose()];
        values.extend(credential.github_git_token());
        let mut secrets = self.network.secrets();
        secrets.extend_from_slice(credential.diagnostic_secrets());
        match credential.git() {
            Some(SourceGitCredential::Https { username, password }) => {
                values.extend([username, password]);
                secrets.push(git_auth::encode_user_password(username, password));
            }
            Some(SourceGitCredential::Ssh {
                private_key,
                known_hosts,
            }) => {
                values.extend([private_key, known_hosts]);
                values.extend(private_key.lines());
            }
            None => {}
        }
        for value in values.into_iter().filter(|value| !value.is_empty()) {
            secrets.extend([
                value.to_owned(),
                credential_segment(value),
                git_auth::encode_basic_credential(value),
            ]);
        }
        secrets
    }

    /// Forge response text is redacted before truncation and kept on one control-free line.
    fn display_text(
        &self,
        bytes: Vec<u8>,
        truncated: bool,
        credential: SourceDeliveryCredential<'_>,
    ) -> String {
        let secrets = self.diagnostic_secrets(credential);
        // Redact every credential before any size bound or lossy UTF-8 conversion can split it.
        let (text, truncated) = command::diagnostic_text_with_secrets(bytes, truncated, &secrets);
        let mut text = text.replace(char::is_control, " ");
        if truncated {
            text.push_str(" [truncated]");
        }
        text
    }
}

// Match the request encoder, including slash and percent escapes, without changing its URL.
fn credential_segment(token: &str) -> String {
    let mut url = Url::parse("https://redacted.invalid/").expect("static hierarchical URL");
    url.path_segments_mut()
        .expect("HTTP URL supports path segments")
        .push(token);
    url.path()[1..].to_owned()
}

/// Forgejo prefixes each job-log line with its timestamp. The delivery diagnostic keeps the line
/// without that prefix; a line that has none is kept unchanged.
pub(super) fn strip_log_timestamps(text: &str) -> String {
    text.lines()
        .map(|line| match timestamp_prefix(line) {
            Some(prefix) => &line[prefix..],
            None => line,
        })
        .collect::<Vec<&str>>()
        .join("\n")
}

/// The length of the timestamp prefix a line begins with, or `None` when it has none: the complete
/// UTC stamp `YYYY-MM-DDTHH:MM:SS`, an optional fractional part with at least one digit, `Z`, and
/// then exactly one space. Numeric positions must hold digits, so a prefix-like line such as
/// `2024-01-02T03:04:05.678 error` is not read as a timestamp.
fn timestamp_prefix(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut end = 0;
    for (width, separator) in [
        (4, Some(b'-')),
        (2, Some(b'-')),
        (2, Some(b'T')),
        (2, Some(b':')),
        (2, Some(b':')),
        (2, None),
    ] {
        if !digit_run(bytes, end, width) {
            return None;
        }
        end += width;
        if let Some(separator) = separator {
            if bytes.get(end) != Some(&separator) {
                return None;
            }
            end += 1;
        }
    }
    if bytes.get(end) == Some(&b'.') {
        let start = end + 1;
        if !digit_run(bytes, start, 1) {
            return None;
        }
        end = start;
        while digit_run(bytes, end, 1) {
            end += 1;
        }
    }
    if bytes.get(end) != Some(&b'Z') {
        return None;
    }
    end += 1;
    (bytes.get(end) == Some(&b' ')).then_some(end + 1)
}

fn digit_run(bytes: &[u8], start: usize, width: usize) -> bool {
    start + width <= bytes.len() && bytes[start..start + width].iter().all(u8::is_ascii_digit)
}

/// Client-side refusals name the forge they belong to, and never a credential.
fn validation_error(message: &str) -> SourceDeliveryError {
    SourceDeliveryError::identity(format!("Forgejo API {message}"))
}

// A URL host keeps its brackets for an IPv6 address, so they are removed before parsing.
#[cfg(test)]
fn loopback_host(host: &str) -> bool {
    host.strip_prefix('[')
        .unwrap_or(host)
        .strip_suffix(']')
        .unwrap_or(host)
        .parse::<std::net::IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

pub(super) fn require_version(version: &str) -> Result<(), SourceDeliveryError> {
    let core = version.split(['+', '-']).next().unwrap_or("");
    let mut parts = core.split('.');
    let parsed = (
        parts.next().and_then(|part| part.parse::<u64>().ok()),
        parts.next().and_then(|part| part.parse::<u64>().ok()),
        parts.next().and_then(|part| part.parse::<u64>().ok()),
    );
    match parsed {
        (Some(major), Some(minor), Some(patch))
            if parts.next().is_none() && (major, minor, patch) >= MINIMUM_VERSION =>
        {
            Ok(())
        }
        _ => Err(SourceDeliveryError::identity(
            "Forgejo server 16.0.0 or newer is required for delivery",
        )),
    }
}
