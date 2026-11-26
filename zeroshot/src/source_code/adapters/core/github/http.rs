//! Bounded GitHub HTTP traffic. The caller owns retry and mutation reconciliation.

use std::fmt;

use openengine_cluster_protocol::SourceForgeBaseUrl;
use reqwest::{Client, ClientBuilder, Method, RequestBuilder, StatusCode, Url, header::HeaderMap};
use serde_json::{Value, json};
use tokio::time::{Instant, timeout_at};
use url::Position;

use super::super::{ci_diagnostics::diagnostic_secrets, network::ForgeNetwork};
use crate::source_code_provider::{SourceDeliveryCredential, SourceDeliveryError};

const MAX_ERROR_BYTES: usize = 64 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 8 * 1024;
const API_VERSION_HEADER: &str = "x-github-api-version";

#[derive(Clone)]
pub(super) struct Http {
    client: Client,
    rest: Url,
    graphql: Url,
    network: ForgeNetwork,
    api_version: &'static str,
}

impl fmt::Debug for Http {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitHub HTTP")
            .finish_non_exhaustive()
    }
}

pub(super) enum Request<'a> {
    Rest {
        method: Method,
        path: &'a [&'a str],
        query: &'a [(&'a str, &'a str)],
        body: Option<&'a Value>,
    },
    Graphql {
        document: &'a str,
        variables: &'a Value,
    },
}

impl<'a> Request<'a> {
    pub(super) fn get(path: &'a [&'a str]) -> Self {
        Self::Rest {
            method: Method::GET,
            path,
            query: &[],
            body: None,
        }
    }
}

pub(super) struct Reply {
    pub head: Head,
    pub body: Vec<u8>,
}

impl fmt::Debug for Reply {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitHub HTTP reply")
            .field("head", &self.head)
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

pub(super) struct Head {
    pub status: StatusCode,
    pub headers: HeaderMap,
}

impl Head {
    pub(super) fn diagnostic(&self) -> String {
        let status = self.status.as_u16();
        let request_id = self
            .headers
            .get("x-github-request-id")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty() && value.len() <= 256);
        match request_id {
            Some(id) => format!("HTTP {status}, GitHub request {id}"),
            None => format!("HTTP {status}"),
        }
    }
}

impl fmt::Debug for Head {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitHub HTTP head")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FailureKind {
    InvalidRequest,
    Connect,
    Send,
    Timeout,
    InterruptedBody,
    Oversize,
    InvalidRedirect,
}

#[derive(Debug)]
pub(super) struct Failure {
    pub kind: FailureKind,
    /// Headers observed before a failed body are diagnostic evidence only.
    pub head: Option<Box<Head>>,
    pub diagnostic: String,
}

impl Failure {
    fn new(kind: FailureKind, head: Option<Head>, diagnostic: String) -> Self {
        Self {
            kind,
            head: head.map(Box::new),
            diagnostic,
        }
    }

    fn invalid_path() -> Self {
        Self::new(
            FailureKind::InvalidRequest,
            None,
            "GitHub API request path is invalid".into(),
        )
    }
}

impl Http {
    pub(super) fn new(
        base: &SourceForgeBaseUrl,
        network: ForgeNetwork,
        api_version: &'static str,
    ) -> Result<Self, SourceDeliveryError> {
        let (rest, graphql) = api_urls(base)?;
        Self::build((rest, graphql), network, api_version)
    }

    fn build(
        (rest, graphql): (Url, Url),
        network: ForgeNetwork,
        api_version: &'static str,
    ) -> Result<Self, SourceDeliveryError> {
        let client =
            build_client(network.client(Client::builder().user_agent("zeroshot-github"))?)?;
        Ok(Self {
            client,
            rest,
            graphql,
            network,
            api_version,
        })
    }

    /// Public review metadata keeps its caller's native HTTP proxy discovery.
    pub(super) fn for_review(
        builder: ClientBuilder,
        api_version: &'static str,
    ) -> Result<Self, SourceDeliveryError> {
        let base = SourceForgeBaseUrl::new("https://github.com").expect("fixed GitHub origin");
        let (rest, graphql) = api_urls(&base)?;
        Ok(Self {
            client: build_client(builder.user_agent("zeroshot-review"))?,
            rest,
            graphql,
            network: ForgeNetwork::default(),
            api_version,
        })
    }

    pub(super) fn rest_base(&self) -> &Url {
        &self.rest
    }

    pub(super) async fn send(
        &self,
        request: Request<'_>,
        credential: SourceDeliveryCredential<'_>,
        budget: (Instant, usize),
    ) -> Result<Reply, Failure> {
        let builder = match request {
            Request::Rest {
                method,
                path,
                query,
                body,
            } => {
                let mut builder = self
                    .client
                    .request(method, self.rest_url(path)?)
                    .query(query);
                if let Some(body) = body {
                    builder = builder.json(body);
                }
                builder
            }
            Request::Graphql {
                document,
                variables,
            } => self
                .client
                .post(self.graphql.clone())
                .json(&json!({"query": document, "variables": variables})),
        };
        self.authenticated(builder, credential, budget).await
    }

    /// Review metadata already encodes its Git references in a relative path.
    pub(super) async fn encoded_get(
        &self,
        path: &str,
        credential: SourceDeliveryCredential<'_>,
        budget: (Instant, usize),
    ) -> Result<Reply, Failure> {
        let mut url = self.rest.clone();
        url.set_path(path);
        self.authenticated(self.client.get(url), credential, budget)
            .await
    }

    async fn authenticated(
        &self,
        builder: RequestBuilder,
        credential: SourceDeliveryCredential<'_>,
        budget: (Instant, usize),
    ) -> Result<Reply, Failure> {
        self.exchange(
            builder
                .bearer_auth(credential.expose())
                .header("accept", "application/vnd.github+json")
                .header(API_VERSION_HEADER, self.api_version),
            budget,
            (
                diagnostic_secrets(self.network.secrets(), credential),
                false,
            ),
        )
        .await
    }

    pub(super) async fn job_log(
        &self,
        (repository, job): (&str, u64),
        credential: SourceDeliveryCredential<'_>,
        budget: (Instant, usize),
    ) -> Result<(Reply, Vec<String>), Failure> {
        let (owner, name) = repository
            .split_once('/')
            .ok_or_else(Failure::invalid_path)?;
        let job = job.to_string();
        let path = ["repos", owner, name, "actions", "jobs", &job, "logs"];
        let reply = self.send(Request::get(&path), credential, budget).await?;
        if !matches!(
            reply.head.status,
            StatusCode::MOVED_PERMANENTLY
                | StatusCode::FOUND
                | StatusCode::SEE_OTHER
                | StatusCode::TEMPORARY_REDIRECT
                | StatusCode::PERMANENT_REDIRECT
        ) {
            return Ok((reply, Vec::new()));
        }
        let Some(url) = log_location(&self.rest_url(&path)?, &reply.head.headers) else {
            return Err(Failure::new(
                FailureKind::InvalidRedirect,
                Some(reply.head),
                "GitHub Actions job log returned an invalid download location".into(),
            ));
        };
        let mut redirect_secrets = vec![url.as_str().to_owned()];
        if let Some(query) = url.query() {
            redirect_secrets.push(query.to_owned());
            redirect_secrets.extend(
                query
                    .split('&')
                    .filter_map(|part| part.split_once('=').map(|(_, value)| value.to_owned())),
            );
            redirect_secrets.extend(url.query_pairs().map(|(_, value)| value.into_owned()));
        }
        let mut secrets = diagnostic_secrets(self.network.secrets(), credential);
        secrets.extend(redirect_secrets.iter().cloned());
        let downloaded = self
            .exchange(self.client.get(url), budget, (secrets, true))
            .await?;
        if downloaded.head.status.is_redirection() {
            return Err(Failure::new(
                FailureKind::InvalidRedirect,
                Some(downloaded.head),
                "GitHub Actions job log download returned another redirect".into(),
            ));
        }
        Ok((downloaded, redirect_secrets))
    }

    fn rest_url(&self, path: &[&str]) -> Result<Url, Failure> {
        if path.is_empty() || path.iter().any(|part| matches!(*part, "" | "." | "..")) {
            return Err(Failure::invalid_path());
        }
        let mut url = self.rest.clone();
        url.path_segments_mut()
            .map_err(|_| Failure::invalid_path())?
            .pop_if_empty()
            .extend(path.iter().copied());
        Ok(url)
    }

    async fn exchange(
        &self,
        builder: RequestBuilder,
        (deadline, maximum): (Instant, usize),
        (secrets, log_download): (Vec<String>, bool),
    ) -> Result<Reply, Failure> {
        let request = builder.build().map_err(|_| {
            Failure::new(
                FailureKind::InvalidRequest,
                None,
                "invalid GitHub HTTP request".into(),
            )
        })?;
        let context = Diagnostic {
            operation: if log_download {
                "GitHub Actions job log download".to_owned()
            } else {
                format!(
                    "GitHub API {} {}",
                    request.method(),
                    &request.url()[..Position::AfterPath]
                )
            },
            secrets,
        };
        if Instant::now() >= deadline {
            return Err(context.deadline(None, Vec::new()));
        }
        let response = timeout_at(deadline, self.client.execute(request))
            .await
            .map_err(|_| context.deadline(None, Vec::new()))?
            .map_err(|error| context.send_failure(error))?;
        read_reply(response, (deadline, maximum), context).await
    }

    #[cfg(test)]
    pub(crate) fn loopback(
        base: &str,
        api_version: &'static str,
    ) -> Result<Self, SourceDeliveryError> {
        let rest = Url::parse(base).map_err(|_| SourceDeliveryError::Rejected)?;
        let loopback = match rest.host() {
            Some(url::Host::Ipv4(address)) => address.is_loopback(),
            Some(url::Host::Ipv6(address)) => address.is_loopback(),
            _ => false,
        };
        if rest.scheme() != "http" || !loopback || !bare_root(&rest) {
            return Err(SourceDeliveryError::Rejected);
        }
        let graphql = rest
            .join("graphql")
            .map_err(|_| SourceDeliveryError::Rejected)?;
        Self::build((rest, graphql), ForgeNetwork::default(), api_version)
    }
}

fn build_client(builder: ClientBuilder) -> Result<Client, SourceDeliveryError> {
    builder
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| SourceDeliveryError::Unavailable)
}

fn api_urls(base: &SourceForgeBaseUrl) -> Result<(Url, Url), SourceDeliveryError> {
    let mut rest = Url::parse(base.as_str()).map_err(|_| SourceDeliveryError::Rejected)?;
    if rest.scheme() != "https" || !bare_root(&rest) {
        return Err(SourceDeliveryError::identity(
            "GitHub requires a root HTTPS instance URL",
        ));
    }
    let host = rest.host_str().ok_or(SourceDeliveryError::Rejected)?;
    let graphql = if host == "github.com" || host.ends_with(".ghe.com") {
        let api_host = format!("api.{host}");
        rest.set_host(Some(&api_host))
            .map_err(|_| SourceDeliveryError::Rejected)?;
        rest.join("graphql")
    } else {
        rest.set_path("/api/v3/");
        rest.join("../graphql")
    }
    .map_err(|_| SourceDeliveryError::Rejected)?;
    Ok((rest, graphql))
}

fn bare_root(url: &Url) -> bool {
    url.path() == "/"
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn log_location(api: &Url, headers: &HeaderMap) -> Option<Url> {
    let mut locations = headers.get_all("location").iter();
    let raw = locations.next()?.to_str().ok()?;
    if raw.is_empty() || raw.chars().any(char::is_control) || locations.next().is_some() {
        return None;
    }
    let download = api.join(raw).ok()?;
    (download.scheme() == "https"
        && download.username().is_empty()
        && download.password().is_none()
        && download.fragment().is_none())
    .then_some(download)
}

enum BodyFailure {
    Oversize,
    Interrupted(reqwest::Error),
}

async fn read_reply(
    mut response: reqwest::Response,
    (deadline, maximum): (Instant, usize),
    context: Diagnostic,
) -> Result<Reply, Failure> {
    let head = Head {
        status: response.status(),
        headers: response.headers().clone(),
    };
    // Redirects are interpreted only through their headers, never a body that may echo a signed URL.
    if head.status.is_redirection() {
        return Ok(Reply {
            head,
            body: Vec::new(),
        });
    }
    let limit = if head.status.is_success() {
        maximum
    } else {
        maximum.min(MAX_ERROR_BYTES)
    };
    let mut body = Vec::new();
    match timeout_at(deadline, read_body(&mut response, &mut body, limit)).await {
        Ok(Ok(())) => Ok(Reply { head, body }),
        Ok(Err(BodyFailure::Oversize)) => Err(context.failure(
            FailureKind::Oversize,
            format!("response exceeded its {limit}-byte bound"),
            (Some(head), body),
        )),
        Ok(Err(BodyFailure::Interrupted(error))) => Err(context.failure(
            FailureKind::InterruptedBody,
            error_detail(error),
            (Some(head), body),
        )),
        Err(_) => Err(context.deadline(Some(head), body)),
    }
}

async fn read_body(
    response: &mut reqwest::Response,
    body: &mut Vec<u8>,
    maximum: usize,
) -> Result<(), BodyFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(BodyFailure::Oversize);
    }
    while let Some(chunk) = response.chunk().await.map_err(BodyFailure::Interrupted)? {
        if body.len().saturating_add(chunk.len()) > maximum {
            body.extend_from_slice(&chunk[..maximum - body.len()]);
            return Err(BodyFailure::Oversize);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(())
}

fn error_detail(error: reqwest::Error) -> String {
    use std::fmt::Write;

    let error = error.without_url();
    let mut detail = error.to_string();
    let mut cause = std::error::Error::source(&error);
    while let Some(current) = cause {
        let _ = write!(detail, ": {current}");
        cause = current.source();
    }
    detail
}

struct Diagnostic {
    operation: String,
    secrets: Vec<String>,
}

impl Diagnostic {
    fn deadline(&self, head: Option<Head>, body: Vec<u8>) -> Failure {
        self.failure(
            FailureKind::Timeout,
            "request timed out at the API deadline".into(),
            (head, body),
        )
    }

    fn send_failure(&self, error: reqwest::Error) -> Failure {
        let kind = if error.is_connect() {
            FailureKind::Connect
        } else if error.is_timeout() {
            FailureKind::Timeout
        } else {
            FailureKind::Send
        };
        self.failure(kind, error_detail(error), (None, Vec::new()))
    }

    fn failure(
        &self,
        kind: FailureKind,
        detail: String,
        (head, mut body): (Option<Head>, Vec<u8>),
    ) -> Failure {
        let status = head
            .as_ref()
            .map_or_else(String::new, |head| format!(" ({})", head.diagnostic()));
        let mut diagnostic = format!("{}{status}: {detail}", self.operation).into_bytes();
        let partial = !body.is_empty();
        if partial {
            diagnostic.extend_from_slice(b"\npartial response: ");
            body.truncate(MAX_ERROR_BYTES);
            diagnostic.extend(body);
        }
        let (mut diagnostic, _) = crate::native_v2_delivery::command::diagnostic_text_with_secrets(
            diagnostic,
            partial,
            &self.secrets,
        );
        let mut boundary = diagnostic.len().min(MAX_DIAGNOSTIC_BYTES);
        while !diagnostic.is_char_boundary(boundary) {
            boundary -= 1;
        }
        diagnostic.truncate(boundary);
        Failure::new(kind, head, diagnostic)
    }
}

#[cfg(all(test, unix))]
#[path = "http/test_scenarios_tests.rs"]
pub(super) mod test_scenarios;
#[cfg(test)]
mod tests;
