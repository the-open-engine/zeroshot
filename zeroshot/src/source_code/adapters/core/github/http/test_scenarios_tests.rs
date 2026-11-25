//! Run the existing scripted scenario state through the real HTTP client in unit tests.
//!
//! Transport tests use raw HTTP fixtures directly. This server preserves scripted
//! test state and request counts while the higher-level cases are ported.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex};

use reqwest::{Method, StatusCode, header::AUTHORIZATION};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use crate::source_code_provider::adapters::core::{http_fixture, network::ForgeNetwork};
use http_fixture::{IntoResponse, Json, Request, Response};

use super::Http;
use super::super::{GhCliDeliveryAuthority, SourceDeliveryError};

pub(in super::super) async fn start(
    authority: &GhCliDeliveryAuthority,
    version: &'static str,
) -> Result<Http, SourceDeliveryError> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| SourceDeliveryError::Unavailable)?;
    let address = listener
        .local_addr()
        .map_err(|_| SourceDeliveryError::Unavailable)?;
    let origin = format!("http://{address}");
    let http = Http::loopback(&origin, version)?;
    let server = Arc::new(Scenario {
        program: authority.config.gh_program.clone(),
        home: authority.config.home_directory.clone(),
        host: authority.hostname(),
        cloud: authority.is_cloud(),
        network: authority.network.clone(),
        origin,
        version,
        pending: Mutex::default(),
    });
    tokio::spawn(http_fixture::serve_async(listener, move |request| {
        let server = server.clone();
        async move { server.respond(request).await }
    }));
    Ok(http)
}

struct Scenario {
    program: PathBuf,
    home: PathBuf,
    host: String,
    cloud: bool,
    network: ForgeNetwork,
    origin: String,
    version: &'static str,
    pending: Mutex<HashMap<String, VecDeque<Response>>>,
}

impl Scenario {
    async fn respond(&self, request: Request) -> Response {
        let Some(call) = Call::parse(&request) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        if let Some(page) = self.next_page(&call) {
            return page;
        }
        if call.following && call.paging == Paging::Graphql {
            return StatusCode::BAD_REQUEST.into_response();
        }
        match self.run(&request, &call).await {
            Some(output) => self.script_response(call, output),
            None => Response::disconnected(),
        }
    }

    fn next_page(&self, call: &Call) -> Option<Response> {
        if !call.following {
            return None;
        }
        let mut pending = self.pending.lock().unwrap();
        let pages = pending.get_mut(&call.key)?;
        let page = pages.pop_front();
        if pages.is_empty() {
            pending.remove(&call.key);
        }
        page
    }

    async fn run(&self, request: &Request, call: &Call) -> Option<Output> {
        let token = request
            .headers
            .get(AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")?;
        let field = if self.cloud {
            "GH_TOKEN"
        } else {
            "GH_ENTERPRISE_TOKEN"
        };
        let mut command = tokio::process::Command::new(&self.program);
        command
            .args(&call.args)
            .args([
                "--header",
                &format!("X-GitHub-Api-Version: {}", self.version),
            ])
            .kill_on_drop(true)
            .env_clear()
            .env("HOME", &self.home)
            .env("GH_HOST", &self.host)
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("GH_PROMPT_DISABLED", "1")
            .env("LANG", "C")
            .env(field, token)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        self.network.apply_command(&mut command);
        run_command(command, &call.input).await
    }

    fn script_response(&self, call: Call, output: Output) -> Response {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let (head, body) = header_prefix(&output.stdout);
        let status = http_status(head).or_else(|| http_status(&stderr));
        if let Some(pages) = call.slurped(body, status) {
            let responses = virtual_pages(&self.origin, &call, pages, |response| {
                complete_reply(response, status.unwrap_or(StatusCode::OK), &stderr)
            });
            return self.save_pages(call.key, responses);
        }
        if output.status.success()
            || status.is_some()
            || serde_json::from_slice::<Value>(body).is_ok()
        {
            return raw_reply(head, body, &stderr, status.unwrap_or(StatusCode::OK));
        }
        if call.path.is_empty() && graphical_error(&stderr) {
            return Json(json!({"errors":[{"message":service_message(&stderr)}]})).into_response();
        }
        if call.mutation && connect_error(&stderr) {
            // Scripted preflight errors stand for retryable, uncertain mutation outcomes.
            // Raw HTTP tests separately verify native socket failure classification.
            return raw_reply("", b"", &stderr, StatusCode::SERVICE_UNAVAILABLE);
        }
        if is_unavailable(&stderr) {
            return Response::disconnected();
        }
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"message": format!("scripted fixture refused request: {stderr}")})),
        )
            .into_response()
    }

    fn save_pages(&self, key: String, mut pages: VecDeque<Response>) -> Response {
        let first = pages
            .pop_front()
            .unwrap_or_else(|| Json(json!([])).into_response());
        let mut pending = self.pending.lock().unwrap();
        pending.remove(&key);
        if !pages.is_empty() {
            pending.insert(key, pages);
        }
        first
    }
}

async fn run_command(mut command: tokio::process::Command, input: &[u8]) -> Option<Output> {
    if input.is_empty() {
        return command.stdin(Stdio::null()).output().await.ok();
    }
    let mut child = command.stdin(Stdio::piped()).spawn().ok()?;
    child.stdin.take()?.write_all(input).await.ok()?;
    child.wait_with_output().await.ok()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Paging {
    None,
    Rest,
    Graphql,
}

struct Call {
    args: Vec<String>,
    input: Vec<u8>,
    key: String,
    path: String,
    paging: Paging,
    following: bool,
    mutation: bool,
}

impl Call {
    fn parse(request: &Request) -> Option<Self> {
        if request.uri.path() == "/graphql" {
            return graphql_call(serde_json::from_slice(&request.body).ok()?);
        }
        Some(rest_call(request))
    }

    fn slurped(&self, bytes: &[u8], status: Option<StatusCode>) -> Option<Vec<Value>> {
        let pages: Vec<Value> = serde_json::from_slice(bytes).ok()?;
        match self.paging {
            Paging::None => None,
            Paging::Rest
                if !pages.first().is_some_and(Value::is_array)
                    && status.is_none_or(|value| value.is_success()) =>
            {
                None
            }
            _ => Some(pages),
        }
    }
}

fn graphql_call(value: Value) -> Option<Call> {
    let document = value["query"].as_str()?;
    let mut variables = value["variables"].as_object()?.clone();
    let cursor = variables.remove("endCursor");
    let paginated = cursor.is_some();
    let following = cursor.is_some_and(|value| !value.is_null());
    let mut args = vec!["api".into(), "graphql".into()];
    if paginated {
        args.extend(["--paginate".into(), "--slurp".into()]);
    }
    args.extend(["-f".into(), format!("query={document}")]);
    let key = format!("{document}{}", Value::Object(variables.clone()));
    append_fields(
        &mut args,
        &mut variables,
        &["owner", "name", "number", "head", "pullRequestId"],
    );
    Some(Call {
        args,
        key,
        following,
        input: Vec::new(),
        path: String::new(),
        mutation: document.trim_start().starts_with("mutation"),
        paging: if paginated {
            Paging::Graphql
        } else {
            Paging::None
        },
    })
}

fn rest_call(request: &Request) -> Call {
    let path = request.uri.path().trim_start_matches('/');
    let query = request.uri.query().unwrap_or_default();
    let page = request
        .uri
        .query_pairs()
        .find(|(name, _)| name == "page")
        .and_then(|(_, value)| value.parse::<usize>().ok());
    let paginated = query.contains("per_page=") && page != Some(1);
    let mut args = vec!["api".into()];
    let endpoint = legacy_endpoint(path, query);
    args.push(endpoint);
    rest_options(&mut args, request, path, paginated);
    let input = rest_body(&mut args, &request.body, path);
    Call {
        args,
        input,
        key: path.into(),
        path: path.into(),
        following: page.is_some_and(|p| p > 1),
        mutation: !matches!(request.method, Method::GET | Method::HEAD),
        paging: if paginated {
            Paging::Rest
        } else {
            Paging::None
        },
    }
}

fn legacy_endpoint(path: &str, query: &str) -> String {
    if path.contains("/git/ref/heads/") {
        return path.replace("%2F", "/");
    }
    if query.is_empty() || path.ends_with("/pulls") {
        return path.into();
    }
    format!("{path}?{query}")
}

fn rest_options(args: &mut Vec<String>, request: &Request, path: &str, paginated: bool) {
    let method = &request.method;
    if *method != Method::GET || !path.contains("/git/ref/heads/") {
        args.extend(["--method".into(), method.to_string()]);
    }
    if path.split('/').count() == 3 && path.starts_with("repos/") {
        args.push("--include".into());
    } else if path.contains("/actions/jobs/") && path.ends_with("/logs") {
        args.push("--allow-escape-sequences".into());
    }
    if paginated {
        args.extend(["--paginate".into(), "--slurp".into()]);
    }
    if path.ends_with("/pulls") {
        for (name, value) in request.uri.query_pairs() {
            args.extend(["-f".into(), format!("{name}={value}")]);
        }
    }
}

fn rest_body(args: &mut Vec<String>, raw: &[u8], path: &str) -> Vec<u8> {
    let Ok(Value::Object(mut values)) = serde_json::from_slice(raw) else {
        return Vec::new();
    };
    if path.contains("/issues/")
        && path.ends_with("/comments")
        && values
            .get("body")
            .and_then(Value::as_str)
            .is_some_and(|body| !body.starts_with("Zeroshot opened pull request #"))
    {
        args.extend(["--input".into(), "-".into()]);
        return raw.to_vec();
    }
    append_fields(args, &mut values, &["title", "body", "head", "base"]);
    Vec::new()
}

fn append_fields(
    args: &mut Vec<String>,
    values: &mut serde_json::Map<String, Value>,
    order: &[&str],
) {
    let selected = order
        .iter()
        .filter_map(|name| values.remove(*name).map(|value| ((*name).into(), value)))
        .collect::<Vec<_>>();
    for (name, value) in selected.into_iter().chain(std::mem::take(values)) {
        let (flag, value) = match value {
            Value::Null => continue,
            Value::String(value) => ("-f", value),
            value => ("-F", value.to_string()),
        };
        args.extend([flag.into(), format!("{name}={value}")]);
    }
}

fn virtual_pages(
    origin: &str,
    call: &Call,
    pages: Vec<Value>,
    last: impl Fn(Response) -> Response,
) -> VecDeque<Response> {
    let length = pages.len();
    pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| {
            let mut response = Json(page).into_response();
            if index + 1 == length {
                response = last(response);
            } else if call.paging == Paging::Rest {
                response = response.header(
                    "Link",
                    &format!(
                        "<{origin}/{}?per_page=100&page={}>; rel=\"next\"",
                        call.path,
                        index + 2
                    ),
                );
            }
            response
        })
        .collect()
}

fn header_prefix(bytes: &[u8]) -> (&str, &[u8]) {
    if bytes.starts_with(b"HTTP/") {
        for ending in [b"\r\n\r\n".as_slice(), b"\n\n".as_slice()] {
            if let Some(offset) = bytes.windows(ending.len()).position(|part| part == ending) {
                let head = std::str::from_utf8(&bytes[..offset]).unwrap_or_default();
                return (head, &bytes[offset + ending.len()..]);
            }
        }
    }
    ("", bytes)
}

fn http_status(text: &str) -> Option<StatusCode> {
    let words = text.split_whitespace().collect::<Vec<_>>();
    words.windows(2).find_map(|parts| {
        let marker = parts[0].trim_start_matches('(');
        if marker != "HTTP" && !marker.starts_with("HTTP/") && marker != "code:" {
            return None;
        }
        let value = parts[1].trim_matches(|ch: char| !ch.is_ascii_digit());
        StatusCode::from_u16(value.parse().ok()?).ok()
    })
}

fn raw_reply(head: &str, body: &[u8], stderr: &str, status: StatusCode) -> Response {
    let mut response = if body.is_empty() && !status.is_success() {
        Json(json!({"message":service_message(stderr)})).into_response()
    } else {
        Response::raw(body.to_vec())
    };
    for (name, value) in head.lines().skip(1).filter_map(|line| line.split_once(':')) {
        if !matches!(
            name.to_ascii_lowercase().as_str(),
            "content-length" | "connection" | "content-type"
        ) {
            response = response.header(name, value.trim());
        }
    }
    complete_reply(response, status, stderr)
}

fn service_message(stderr: &str) -> &str {
    let text = stderr.trim();
    let text = text.strip_prefix("gh: ").unwrap_or(text);
    if let Some(detail) = text.strip_prefix("HTTP ")
        && let Some((_, message)) = detail.split_once(": ")
    {
        return message;
    }
    text.rsplit_once(" (HTTP ")
        .filter(|(_, suffix)| suffix.ends_with(')'))
        .map_or(text, |(message, _)| message)
}

fn complete_reply(response: Response, status: StatusCode, error: &str) -> Response {
    let response = response.status(status);
    if status != StatusCode::FORBIDDEN {
        return response;
    }
    let message = service_message(error);
    if message.starts_with("API rate limit exceeded") {
        return response.header("x-ratelimit-remaining", "0");
    }
    if message.starts_with("You have exceeded a secondary rate limit") {
        return response.header("retry-after", "1");
    }
    response
}

fn graphical_error(stderr: &str) -> bool {
    let message = service_message(stderr);
    message.starts_with("GraphQL: ")
        || message.starts_with("Something went wrong while executing your query on ")
}

fn connect_error(stderr: &str) -> bool {
    stderr.contains("error connecting to ") || stderr.contains("TLS handshake timeout")
}

fn is_unavailable(stderr: &str) -> bool {
    connect_error(stderr)
        || ["unexpected EOF", "unexpected end of JSON input"]
            .iter()
            .any(|message| stderr.contains(message))
}
