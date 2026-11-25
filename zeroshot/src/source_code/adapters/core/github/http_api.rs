use std::collections::BTreeSet;

use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};
use tokio::time::{Instant, sleep, timeout_at};

use super::api::{MutationFailure, MutationRejection};
use super::http::{Failure, FailureKind, Http, Reply, Request};
use super::super::ci_diagnostics::{MAX_JOB_LOG_BYTES, diagnostic_secrets, redact_log};
use super::*;

const API_VERSION: &str = "2026-03-10";
const MAX_PAGES: usize = 1_000;
const MAX_MUTATION_RESPONSE_BYTES: usize = 16 * 1024;

pub(super) const PAGE_SIZE: &[(&str, &str)] = &[("per_page", "100")];

#[derive(Clone, Copy)]
pub(super) enum GraphqlRead {
    Once,
    Policy,
}

#[derive(Clone, Copy)]
pub(super) struct GraphqlPages<'a> {
    pub document: &'a str,
    pub cursor_path: &'a [&'a str],
    pub read: GraphqlRead,
}

impl GhCliDeliveryAuthority {
    #[cfg(test)]
    pub(crate) fn with_test_api(mut self, address: &str) -> Self {
        self.http = Arc::new(tokio::sync::OnceCell::new_with(Some(
            Http::loopback(address, API_VERSION).expect("loopback-only API fixture"),
        )));
        self
    }

    pub(super) async fn http(&self) -> Result<&Http, SourceDeliveryError> {
        self.http
            .get_or_try_init(|| async {
                #[cfg(all(test, unix))]
                if self.config.gh_program != std::path::Path::new("/usr/bin/gh")
                    && self.config.gh_program != std::path::Path::new("gh")
                {
                    return super::http::test_scenarios::start(self, API_VERSION).await;
                }
                Http::new(&self.base_url, self.network.clone(), API_VERSION)
            })
            .await
    }

    pub(super) fn api_deadline(&self) -> Instant {
        Instant::now() + self.config.api_deadline
    }

    pub(super) async fn rest(
        &self,
        request: Request<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Value, SourceDeliveryError> {
        self.rest_at(request, credential, (MAX_JOB_LOG_BYTES, false))
            .await
    }

    /// Restart a bound PR synchronization from its read when its reply is lost.
    pub(super) async fn rest_review_sync(
        &self,
        request: Request<'_>,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Value, SourceDeliveryError> {
        self.rest_at(request, credential, (MAX_JOB_LOG_BYTES, true))
            .await
    }

    pub(super) async fn bounded_json(
        &self,
        request: Request<'_>,
        credential: SourceDeliveryCredential<'_>,
        maximum: usize,
    ) -> Result<Value, SourceDeliveryError> {
        self.rest_at(request, credential, (maximum, false)).await
    }

    async fn rest_at(
        &self,
        request: Request<'_>,
        credential: SourceDeliveryCredential<'_>,
        (maximum, review_sync): (usize, bool),
    ) -> Result<Value, SourceDeliveryError> {
        self.ensure_instance(credential).await?;
        let read = matches!(&request, Request::Rest { method, .. } if method == Method::GET);
        let recover = |failure: ReadFailure| {
            if review_sync {
                failure.for_review_sync()
            } else if read {
                failure.for_read()
            } else {
                failure.error
            }
        };
        let reply = self
            .http()
            .await?
            .send(request, credential, (self.api_deadline(), maximum))
            .await
            .map_err(|failure| recover(self.read_failure(failure, credential)))?;
        self.http_json(&reply, credential).map_err(recover)
    }

    pub(super) async fn rest_pages(
        &self,
        path: &[&str],
        credential: SourceDeliveryCredential<'_>,
        free_branch_rules: bool,
    ) -> Result<Value, SourceDeliveryError> {
        self.ensure_instance(credential).await?;
        let http = self.http().await?;
        let deadline = self.api_deadline();
        let mut remaining = MAX_JOB_LOG_BYTES;
        let mut pages = Vec::new();
        for page in 1..=MAX_PAGES {
            let page_number = page.to_string();
            let query = if page == 1 {
                PAGE_SIZE
            } else {
                &[("per_page", "100"), ("page", &page_number)]
            };
            let reply = http
                .send(
                    Request::Rest {
                        method: Method::GET,
                        path,
                        query,
                        body: None,
                    },
                    credential,
                    (deadline, remaining),
                )
                .await
                .map_err(|error| self.read_failure(error, credential).for_read())?;
            remaining -= reply.body.len();
            let (value, next) = self.rest_page(
                &reply,
                (http.rest_base(), page, free_branch_rules),
                credential,
            )?;
            pages.push(value);
            if !next {
                return Ok(Value::Array(pages));
            }
        }
        Err(pagination_error())
    }

    fn rest_page(
        &self,
        reply: &Reply,
        (base, page, free_branch_rules): (&url::Url, usize, bool),
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(Value, bool), SourceDeliveryError> {
        if free_branch_rules && page == 1 && branch_rules_unavailable(reply) {
            return Ok((json!([]), false));
        }
        let value = self
            .http_json(reply, credential)
            .map_err(ReadFailure::for_read)?;
        if !value.is_array() {
            return Err(SourceDeliveryError::api(
                None,
                "GitHub REST page is not an array",
            ));
        }
        Ok((value, has_next_rest_page(reply, base, page)?))
    }

    pub(super) async fn graphql(
        &self,
        document: &str,
        variables: &Value,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Value, SourceDeliveryError> {
        self.ensure_instance(credential).await?;
        self.graphql_page(
            Request::Graphql {
                document,
                variables,
            },
            credential,
            (self.api_deadline(), MAX_JOB_LOG_BYTES),
        )
        .await
        .map(|(value, _)| value)
        .map_err(ReadFailure::for_read)
    }

    pub(super) async fn graphql_pages(
        &self,
        query: GraphqlPages<'_>,
        variables: Value,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Value, SourceDeliveryError> {
        self.ensure_instance(credential).await?;
        let deadline = self.api_deadline();
        for attempt in 1..=3 {
            let failure = match self
                .read_graphql_pages(query, variables.clone(), (credential, deadline))
                .await
            {
                Ok(value) => return Ok(value),
                Err(failure) => failure,
            };
            match (query.read, failure.recovery, attempt) {
                (GraphqlRead::Once, _, _)
                | (_, ReadRecovery::None, _)
                | (_, ReadRecovery::Deadline, 1) => return Err(failure.for_read()),
                (_, ReadRecovery::Deadline, _) => {}
                (_, _, 3) => {
                    return Err(self.policy_exhausted(
                        failure.error,
                        "after 3 attempts",
                        credential,
                    ));
                }
                _ => {
                    if timeout_at(deadline, sleep(Duration::from_secs(attempt)))
                        .await
                        .is_ok()
                    {
                        continue;
                    }
                }
            }
            return Err(self.policy_exhausted(failure.error, "at the API deadline", credential));
        }
        unreachable!("policy read returns within its attempt bound")
    }

    async fn read_graphql_pages(
        &self,
        query: GraphqlPages<'_>,
        mut variables: Value,
        (credential, deadline): (SourceDeliveryCredential<'_>, Instant),
    ) -> Result<Value, ReadFailure> {
        let vars = variables
            .as_object_mut()
            .ok_or(SourceDeliveryError::Rejected)?;
        vars.insert("endCursor".to_owned(), Value::Null);
        let mut remaining = MAX_JOB_LOG_BYTES;
        let mut pages = Vec::new();
        let mut cursors = BTreeSet::new();
        for _ in 0..MAX_PAGES {
            let (page, bytes) = self
                .graphql_page(
                    Request::Graphql {
                        document: query.document,
                        variables: &variables,
                    },
                    credential,
                    (deadline, remaining),
                )
                .await?;
            remaining -= bytes;
            let next = graphql_cursor(&page, query.cursor_path, pages.is_empty())?;
            match next {
                None => {
                    pages.push(page);
                    return Ok(Value::Array(pages));
                }
                Some(cursor) if cursors.insert(cursor.to_owned()) => {
                    variables["endCursor"] = Value::String(cursor.to_owned());
                    pages.push(page);
                }
                Some(_) => return Err(pagination_error().into()),
            }
        }
        Err(pagination_error().into())
    }

    async fn graphql_page(
        &self,
        request: Request<'_>,
        credential: SourceDeliveryCredential<'_>,
        budget: (Instant, usize),
    ) -> Result<(Value, usize), ReadFailure> {
        let reply = self
            .http()
            .await?
            .send(request, credential, budget)
            .await
            .map_err(|failure| self.read_failure(failure, credential))?;
        let value = self.http_json(&reply, credential)?;
        if !complete_graphql(&value) {
            return Err(ReadFailure {
                error: self.body_error(
                    &reply,
                    ("GitHub GraphQL read failed", None),
                    (credential, &[]),
                ),
                recovery: if generic_query_failure(&value) {
                    ReadRecovery::QueryExecution
                } else {
                    ReadRecovery::None
                },
            });
        }
        Ok((value, reply.body.len()))
    }

    pub(super) async fn mutation(
        &self,
        (document, field): (&str, &str),
        variables: &Value,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Value, MutationFailure> {
        self.ensure_instance(credential)
            .await
            .map_err(MutationFailure::not_sent)?;
        let reply = self
            .http()
            .await
            .map_err(MutationFailure::not_sent)?
            .send(
                Request::Graphql {
                    document,
                    variables,
                },
                credential,
                (self.api_deadline(), MAX_MUTATION_RESPONSE_BYTES),
            )
            .await
            .map_err(|failure| {
                let before_send = matches!(
                    failure.kind,
                    FailureKind::Connect | FailureKind::InvalidRequest
                );
                let error = self.http_failure(failure, credential);
                if before_send {
                    MutationFailure::not_sent(error)
                } else {
                    MutationFailure::unknown(error)
                }
            })?;
        if reply.head.status.is_redirection() {
            return Err(MutationFailure::unknown(self.body_error(
                &reply,
                ("GitHub redirected a mutation", None),
                (credential, &[]),
            )));
        }
        let value = self
            .http_json(&reply, credential)
            .map_err(|failure| MutationFailure::unknown(failure.error))?;
        if complete_graphql(&value) && value["data"].get(field).is_some_and(Value::is_object) {
            return Ok(value);
        }
        Err(MutationFailure {
            error: self.body_error(
                &reply,
                ("GitHub GraphQL mutation failed", None),
                (credential, &[]),
            ),
            rejection: mutation_rejection(&value, field),
        })
    }

    pub(super) fn policy_exhausted(
        &self,
        error: SourceDeliveryError,
        reason: &str,
        credential: SourceDeliveryCredential<'_>,
    ) -> SourceDeliveryError {
        let diagnostic = format!("GitHub policy read retry allowance exhausted {reason}.\n{error}");
        SourceDeliveryError::api(
            error.api_status(),
            redact_log(&diagnostic, credential, &self.network.secrets()),
        )
        .exhausted()
    }

    fn read_failure(
        &self,
        failure: Failure,
        credential: SourceDeliveryCredential<'_>,
    ) -> ReadFailure {
        let recovery = match failure.kind {
            FailureKind::Timeout => ReadRecovery::Deadline,
            FailureKind::Send | FailureKind::InterruptedBody => ReadRecovery::LostReply,
            _ => ReadRecovery::None,
        };
        ReadFailure {
            error: self.http_failure(failure, credential),
            recovery,
        }
    }

    pub(super) fn http_failure(
        &self,
        failure: Failure,
        credential: SourceDeliveryCredential<'_>,
    ) -> SourceDeliveryError {
        let error = SourceDeliveryError::api(
            None,
            redact_log(&failure.diagnostic, credential, &self.network.secrets()),
        );
        if failure.kind == FailureKind::Connect && failure.head.is_none() {
            error.temporary()
        } else {
            error
        }
    }

    pub(super) fn status_error(
        &self,
        reply: &Reply,
        credential: SourceDeliveryCredential<'_>,
        secrets: &[String],
    ) -> SourceDeliveryError {
        let status = reply.head.status;
        let error = self.body_error(
            reply,
            ("GitHub API request failed", Some(status)),
            (credential, secrets),
        );
        if transient_status(reply) {
            error.temporary()
        } else {
            error
        }
    }

    pub(super) fn http_json(
        &self,
        reply: &Reply,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<Value, ReadFailure> {
        if !reply.head.status.is_success() {
            return Err(self.status_error(reply, credential, &[]).into());
        }
        serde_json::from_slice(&reply.body).map_err(|error| ReadFailure {
            recovery: if error.is_eof() && !reply.body.is_empty() {
                ReadRecovery::LostReply
            } else {
                ReadRecovery::None
            },
            error: self.body_error(
                reply,
                (&format!("GitHub returned invalid JSON: {error}"), None),
                (credential, &[]),
            ),
        })
    }

    pub(super) fn body_error(
        &self,
        reply: &Reply,
        (message, authority): (&str, Option<StatusCode>),
        (credential, extra): (SourceDeliveryCredential<'_>, &[String]),
    ) -> SourceDeliveryError {
        let mut secrets = self.network.secrets();
        secrets.extend_from_slice(extra);
        let secrets = diagnostic_secrets(secrets, credential);
        let limit = reply.body.len().min(64 * 1024);
        let (body, _) = crate::native_v2_delivery::command::diagnostic_text_with_secrets(
            reply.body[..limit].to_vec(),
            limit < reply.body.len(),
            &secrets,
        );
        let context = format!("{message} ({})", reply.head.diagnostic());
        let body = super::super::ci_diagnostics::bounded_text(&body, 8 * 1024);
        SourceDeliveryError::api(
            authority.map(|status| status.as_u16()),
            redact_log(&format!("{context}\n{body}"), credential, &secrets),
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadRecovery {
    None,
    LostReply,
    Deadline,
    QueryExecution,
}

pub(super) struct ReadFailure {
    pub(super) error: SourceDeliveryError,
    recovery: ReadRecovery,
}

impl ReadFailure {
    fn for_read(self) -> SourceDeliveryError {
        if matches!(
            self.recovery,
            ReadRecovery::LostReply | ReadRecovery::Deadline
        ) {
            self.error.temporary()
        } else {
            self.error
        }
    }

    fn for_review_sync(self) -> SourceDeliveryError {
        if matches!(
            self.recovery,
            ReadRecovery::LostReply | ReadRecovery::Deadline
        ) {
            self.error.temporary_review_sync()
        } else {
            self.error
        }
    }
}

impl From<SourceDeliveryError> for ReadFailure {
    fn from(error: SourceDeliveryError) -> Self {
        Self {
            error,
            recovery: ReadRecovery::None,
        }
    }
}

fn complete_graphql(value: &Value) -> bool {
    value.get("data").is_some_and(Value::is_object)
        && value
            .get("errors")
            .is_none_or(|errors| errors.as_array().is_some_and(Vec::is_empty))
}

fn pagination_error() -> SourceDeliveryError {
    SourceDeliveryError::api(None, "GitHub pagination is incomplete or did not advance")
}

fn graphql_cursor<'a>(
    value: &'a Value,
    path: &[&str],
    first: bool,
) -> Result<Option<&'a str>, SourceDeliveryError> {
    let node = path.iter().try_fold(value, |value, key| {
        if let Some(array) = value.as_array() {
            key.parse::<usize>().ok().and_then(|index| array.get(index))
        } else {
            value.get(*key)
        }
    });
    match node.filter(|value| !value.is_null()) {
        None if first => Ok(None),
        Some(node) if node["hasNextPage"].as_bool() == Some(false) => Ok(None),
        Some(node) if node["hasNextPage"].as_bool() == Some(true) => node["endCursor"]
            .as_str()
            .filter(|cursor| !cursor.is_empty())
            .map(Some)
            .ok_or_else(pagination_error),
        _ => Err(pagination_error()),
    }
}

fn has_next_rest_page(
    reply: &Reply,
    api_base: &url::Url,
    current: usize,
) -> Result<bool, SourceDeliveryError> {
    let mut next = false;
    for header in reply.head.headers.get_all(header::LINK) {
        if let Some(target) = next_link_target(header.to_str().map_err(|_| pagination_error())?)? {
            if next {
                return Err(pagination_error());
            }
            validate_page_target(api_base, target, current)?;
            next = true;
        }
    }
    Ok(next)
}

fn next_link_target(link: &str) -> Result<Option<&str>, SourceDeliveryError> {
    let mut next = None;
    let mut remaining = Some(link);
    while let Some(entry) = remaining {
        let (target, parameter) = entry
            .trim()
            .strip_prefix('<')
            .and_then(|entry| entry.split_once('>'))
            .filter(|(target, _)| !target.is_empty() && !target.contains(['<', ' ', '\t']))
            .ok_or_else(pagination_error)?;
        let (parameter, rest) = match parameter.split_once(',') {
            Some((parameter, rest)) => (parameter, Some(rest)),
            None => (parameter, None),
        };
        remaining = rest;
        match link_relation(parameter)? {
            "next" if next.is_none() => next = Some(target),
            "first" | "last" | "prev" => {}
            _ => return Err(pagination_error()),
        }
    }
    Ok(next)
}

fn link_relation(parameter: &str) -> Result<&str, SourceDeliveryError> {
    // GitHub emits one pagination relation in each entry. A broken relation
    // cannot establish that the page was the last one.
    let relation = parameter
        .trim()
        .strip_prefix(';')
        .and_then(|parameter| parameter.split_once('='))
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("rel"))
        .map(|(_, relation)| relation.trim())
        .ok_or_else(pagination_error)?;
    Ok(relation
        .strip_prefix('"')
        .and_then(|quoted| quoted.strip_suffix('"'))
        .unwrap_or(relation))
}

fn validate_page_target(
    api_base: &url::Url,
    target: &str,
    current: usize,
) -> Result<(), SourceDeliveryError> {
    // Read only the page number. Constructors retain the authored resource and filters even
    // when GitHub's Link uses its numeric repository URL.
    let url = api_base.join(target).map_err(|_| pagination_error())?;
    let mut pages = url.query_pairs().filter(|(name, _)| name == "page");
    let page = pages
        .next()
        .and_then(|(_, value)| value.parse::<usize>().ok());
    (url.origin() == api_base.origin()
        && url.fragment().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && page == current.checked_add(1)
        && pages.next().is_none())
    .then_some(())
    .ok_or_else(pagination_error)
}

fn branch_rules_unavailable(reply: &Reply) -> bool {
    reply.head.status == StatusCode::FORBIDDEN
        && !reply.head.headers.contains_key(header::LINK)
        && serde_json::from_slice::<Value>(&reply.body).ok().is_some_and(|value| {
            value["message"].as_str() == Some("Upgrade to GitHub Pro or make this repository public to enable this feature.")
                && value.get("status").is_none_or(|status| status.as_u64() == Some(403) || status.as_str() == Some("403"))
        })
}

fn rate_limited(reply: &Reply) -> bool {
    reply
        .head
        .headers
        .get("x-ratelimit-remaining")
        .is_some_and(|value| value == "0")
        || reply.head.headers.get(header::RETRY_AFTER).is_some()
        || serde_json::from_slice::<Value>(&reply.body)
            .ok()
            .is_some_and(|value| {
                value["message"].as_str().is_some_and(|message| {
                    message.starts_with("API rate limit exceeded")
                        || message.starts_with("You have exceeded a secondary rate limit")
                })
            })
}

pub(super) fn transient_status(reply: &Reply) -> bool {
    let status = reply.head.status;
    status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
        || status == StatusCode::FORBIDDEN && rate_limited(reply)
}

fn generic_query_failure(value: &Value) -> bool {
    let Some(errors) = value
        .as_object()
        .filter(|value| value.len() == 1)
        .and_then(|value| value.get("errors"))
        .and_then(Value::as_array)
        .filter(|errors| !errors.is_empty())
    else {
        return false;
    };
    errors.iter().all(|error| {
        error
            .as_object()
            .filter(|error| error.len() == 1)
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .is_some_and(execution_error)
    })
}

fn execution_error(message: &str) -> bool {
    let Some(detail) = message
        .strip_prefix("Something went wrong while executing your query on ")
        .and_then(|text| text.strip_suffix("` when reporting this issue."))
    else {
        return false;
    };
    let Some((timestamp, request_id)) = detail.split_once(". Please include `") else {
        return false;
    };
    [timestamp, request_id].into_iter().all(|value| {
        !value.is_empty()
            && value
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && byte != b'`')
    })
}

fn mutation_rejection(value: &Value, mutation: &str) -> Option<MutationRejection> {
    let errors = definitive_rejection(value, mutation)?;
    if errors.iter().all(|error| {
        error["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("A conversation must be resolved"))
    }) {
        Some(MutationRejection::Conversation)
    } else if mutation == "enqueuePullRequest"
        && errors.iter().all(|error| {
            error["message"]
                .as_str()
                .and_then(|message| message.strip_prefix("Required status check \""))
                .and_then(|name| name.strip_suffix("\" is queued."))
                .is_some_and(|name| !name.is_empty())
        })
    {
        Some(MutationRejection::RequiredCheckQueued)
    } else {
        Some(MutationRejection::Policy)
    }
}

fn definitive_rejection<'a>(value: &'a Value, mutation: &str) -> Option<&'a [Value]> {
    let data = value.get("data")?.as_object()?;
    if data.len() != 1 || !data.get(mutation).is_some_and(Value::is_null) {
        return None;
    }
    let path = json!([mutation]);
    let errors = value.get("errors")?.as_array()?;
    if errors.is_empty()
        || !errors
            .iter()
            .all(|error| error["type"].as_str() == Some("UNPROCESSABLE") && error["path"] == path)
    {
        return None;
    }
    Some(errors)
}

pub(super) fn repository_path<'a>(
    repository: &'a str,
    tail: &[&'a str],
) -> Result<Vec<&'a str>, SourceDeliveryError> {
    let (owner, name) = repository
        .split_once('/')
        .ok_or(SourceDeliveryError::Rejected)?;
    let mut path = vec!["repos", owner, name];
    path.extend_from_slice(tail);
    Ok(path)
}
