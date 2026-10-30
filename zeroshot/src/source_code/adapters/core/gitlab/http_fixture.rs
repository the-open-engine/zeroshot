//! Minimal loopback-only HTTP fixture; no production endpoint override is exposed.
use std::sync::Arc;
use reqwest::{Method, StatusCode, Url, header::HeaderMap};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) struct Request {
    pub method: Method,
    pub uri: Url,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

pub(super) struct Response {
    status: StatusCode,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}

impl Response {
    pub(super) fn header(mut self, name: &str, value: &str) -> Self {
        assert!(!name.contains(['\r', '\n']) && !value.contains(['\r', '\n']));
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

pub(super) trait IntoResponse {
    fn into_response(self) -> Response;
}

pub(super) struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        Response {
            status: StatusCode::OK,
            headers: Vec::new(),
            body: serde_json::to_vec(&self.0).unwrap(),
        }
    }
}

impl IntoResponse for StatusCode {
    fn into_response(self) -> Response {
        Response {
            status: self,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
}

impl<T: Serialize> IntoResponse for (StatusCode, Json<T>) {
    fn into_response(self) -> Response {
        Response {
            status: self.0,
            headers: Vec::new(),
            body: serde_json::to_vec(&self.1.0).unwrap(),
        }
    }
}

pub(super) async fn serve(
    listener: tokio::net::TcpListener,
    handler: impl Fn(Request) -> Response + Send + Sync + 'static,
) {
    let handler = Arc::new(handler);
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let handler = handler.clone();
        tokio::spawn(async move {
            let response = match read(&mut stream).await {
                Some(request) => handler(request),
                None => StatusCode::BAD_REQUEST.into_response(),
            };
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
        });
    }
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

fn parse_header(header: &str) -> Option<(Method, Url, HeaderMap, usize)> {
    let mut lines = header.split("\r\n");
    let mut request = lines.next()?.split_whitespace();
    let method = request.next()?.parse().ok()?;
    let uri = Url::parse(&format!("http://127.0.0.1{}", request.next()?)).ok()?;
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
