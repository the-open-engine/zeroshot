//! Test-only loopback HTTP fixture. No production endpoint override is exposed.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use reqwest::{Method, StatusCode, header::HeaderMap};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

#[derive(Clone, Debug)]
pub(super) struct Request {
    pub method: Method,
    pub uri: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

#[derive(Debug)]
pub(super) struct Response {
    status: StatusCode,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}

impl Response {
    pub(super) fn status(status: StatusCode) -> Self {
        Self {
            status,
            body: Vec::new(),
            headers: Vec::new(),
        }
    }

    pub(super) fn json<T: Serialize>(value: T) -> Self {
        Self {
            status: StatusCode::OK,
            body: serde_json::to_vec(&value).unwrap(),
            headers: Vec::new(),
        }
    }

    pub(super) fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    pub(super) fn header(mut self, name: &str, value: &str) -> Self {
        assert!(!name.contains(['\r', '\n']) && !value.contains(['\r', '\n']));
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

pub(super) struct Fixture {
    endpoint: String,
    pending: Arc<Mutex<VecDeque<Response>>>,
    requests: Arc<Mutex<Vec<Request>>>,
    server: JoinHandle<()>,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let pending = Arc::new(Mutex::new(VecDeque::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handler = {
            let pending = Arc::clone(&pending);
            let requests = Arc::clone(&requests);
            move |request: Request| {
                requests.lock().unwrap().push(request.clone());
                pending
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| Response::status(StatusCode::NOT_FOUND))
            }
        };
        Self {
            endpoint,
            pending,
            requests,
            server: tokio::spawn(serve(listener, handler)),
        }
    }

    pub(super) fn queue(&self, response: Response) -> &Self {
        self.pending.lock().unwrap().push_back(response);
        self
    }

    pub(super) fn endpoint(&self) -> String {
        self.endpoint.clone()
    }

    pub(super) fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn serve(
    listener: tokio::net::TcpListener,
    handler: impl Fn(Request) -> Response + Send + Sync + 'static,
) {
    let handler = Arc::new(handler);
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let handler = Arc::clone(&handler);
        tokio::spawn(async move {
            respond(stream, handler).await;
        });
    }
}

async fn respond(mut stream: tokio::net::TcpStream, handler: Arc<impl Fn(Request) -> Response>) {
    let Some(request) = read(&mut stream).await else {
        return;
    };
    let response = handler(request);
    let extras = response
        .headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    let header = format!(
        "HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extras}\r\n",
        response.status.as_u16(),
        response.body.len()
    );
    if stream.write_all(header.as_bytes()).await.is_ok() {
        // A bounded client may intentionally stop reading an oversized fixture response.
        let _ = stream.write_all(&response.body).await;
    }
    let _ = stream.shutdown().await;
}

async fn read(stream: &mut tokio::net::TcpStream) -> Option<Request> {
    let mut bytes = Vec::new();
    let boundary = read_header(stream, &mut bytes).await?;
    let header = std::str::from_utf8(&bytes[..boundary]).ok()?;
    let (method, uri, headers, length) = parse_header(header)?;
    while bytes.len() - boundary < length {
        let mut chunk = [0_u8; 4096];
        let count = stream.read(&mut chunk).await.ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Some(Request {
        method,
        uri,
        headers,
        body: bytes[boundary..boundary + length].to_vec(),
    })
}

fn parse_header(header: &str) -> Option<(Method, String, HeaderMap, usize)> {
    let mut lines = header.split("\r\n");
    let mut request = lines.next()?.split_whitespace();
    let method = request.next()?.parse().ok()?;
    let uri = request.next()?.to_owned();
    let mut headers = HeaderMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':')?;
        headers.insert(
            reqwest::header::HeaderName::from_bytes(name.as_bytes()).ok()?,
            value.trim().parse().ok()?,
        );
    }
    let length = headers
        .get("content-length")
        .map_or(Some(0), |value| value.to_str().ok()?.parse::<usize>().ok())?;
    (length <= 2 * 1024 * 1024).then_some((method, uri, headers, length))
}

async fn read_header(stream: &mut tokio::net::TcpStream, bytes: &mut Vec<u8>) -> Option<usize> {
    loop {
        let mut chunk = [0_u8; 4096];
        let count = stream.read(&mut chunk).await.ok()?;
        if count == 0 || bytes.len() + count > 2 * 1024 * 1024 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(offset) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            return Some(offset + 4);
        }
    }
}
