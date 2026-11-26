//! Minimal loopback-only HTTP fixture; no production endpoint override is exposed.
use std::sync::Arc;
use reqwest::{Method, StatusCode, Url, header::HeaderMap};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(unix)]
use tokio::net::TcpListener;

#[cfg(unix)]
pub(crate) async fn loopback() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    (listener, origin)
}

pub(crate) fn loopback_tls(ca_file: &std::path::Path) -> tokio_rustls::TlsAcceptor {
    let mut ca_params = rcgen::CertificateParams::new(vec![]).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let ca = ca_params.self_signed(&ca_key).unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(vec!["127.0.0.1".into()]).unwrap();
    let cert = params.signed_by(&key, &ca, &ca_key).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone(), ca.der().clone()],
        rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap(),
    )
    .unwrap();
    std::fs::write(ca_file, ca.pem()).unwrap();
    tokio_rustls::TlsAcceptor::from(Arc::new(config))
}

pub(crate) struct Request {
    pub method: Method,
    pub uri: Url,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

pub(crate) struct Response {
    status: StatusCode,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
    content_length: Option<usize>,
    disconnect: bool,
    await_disconnect: Option<Arc<tokio::sync::Notify>>,
}

impl Response {
    pub(crate) fn replace_origin(mut self, previous: &str, origin: &str) -> Self {
        self.body = String::from_utf8(self.body)
            .unwrap()
            .replace(previous, origin)
            .into_bytes();
        self
    }
    pub(crate) fn raw(body: Vec<u8>) -> Self {
        Self {
            status: StatusCode::OK,
            body,
            headers: Vec::new(),
            content_length: None,
            disconnect: false,
            await_disconnect: None,
        }
    }

    pub(crate) fn disconnected() -> Self {
        Self {
            disconnect: true,
            ..Self::raw(Vec::new())
        }
    }

    pub(crate) fn stall_until_disconnect(disconnected: Arc<tokio::sync::Notify>) -> Self {
        Self {
            await_disconnect: Some(disconnected),
            ..Self::raw(Vec::new())
        }
    }

    pub(crate) fn header(mut self, name: &str, value: &str) -> Self {
        assert!(!name.contains(['\r', '\n']) && !value.contains(['\r', '\n']));
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub(crate) fn status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    /// Advertise a length larger than the body to simulate a dropped response mid-frame.
    pub(crate) fn content_length(mut self, length: usize) -> Self {
        assert!(length > self.body.len());
        self.content_length = Some(length);
        self
    }
}

pub(crate) trait IntoResponse {
    fn into_response(self) -> Response;
}

pub(crate) struct Json<T>(pub T);

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        Response::raw(serde_json::to_vec(&self.0).unwrap())
    }
}

impl IntoResponse for StatusCode {
    fn into_response(self) -> Response {
        Response {
            status: self,
            ..Response::raw(Vec::new())
        }
    }
}

impl<T: Serialize> IntoResponse for (StatusCode, Json<T>) {
    fn into_response(self) -> Response {
        Response {
            status: self.0,
            ..self.1.into_response()
        }
    }
}

pub(crate) async fn serve(
    listener: tokio::net::TcpListener,
    handler: impl Fn(Request) -> Response + Send + Sync + 'static,
) {
    serve_async(listener, move |request| {
        std::future::ready(handler(request))
    })
    .await;
}

pub(crate) async fn serve_async<Fut>(
    listener: tokio::net::TcpListener,
    handler: impl Fn(Request) -> Fut + Send + Sync + 'static,
) where
    Fut: Future<Output = Response> + Send + 'static,
{
    let handler = Arc::new(handler);
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let handler = handler.clone();
        tokio::spawn(async move {
            respond(stream, handler).await;
        });
    }
}

pub(crate) async fn serve_tls(
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    handler: impl Fn(Request) -> Response + Send + Sync + 'static,
) {
    let handler = Arc::new(move |request| std::future::ready(handler(request)));
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let handler = handler.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            if let Ok(stream) = acceptor.accept(stream).await {
                respond(stream, handler).await;
            }
        });
    }
}

async fn respond<S, Fut>(mut stream: S, handler: Arc<impl Fn(Request) -> Fut>)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    Fut: Future<Output = Response>,
{
    let response = match read(&mut stream).await {
        Some(request) => handler(request).await,
        None => StatusCode::BAD_REQUEST.into_response(),
    };
    if response.disconnect {
        return;
    }
    if let Some(disconnected) = response.await_disconnect {
        let result = stream.read(&mut [0]).await;
        assert!(!matches!(result, Ok(1)), "client sent an unexpected byte");
        disconnected.notify_one();
        return;
    }
    let extras = response
        .headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    let header = format!(
        "HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extras}\r\n",
        response.status.as_u16(),
        response.content_length.unwrap_or(response.body.len())
    );
    if stream.write_all(header.as_bytes()).await.is_ok() {
        // A bounded client may intentionally stop reading an oversized fixture response.
        let _ = stream.write_all(&response.body).await;
    }
}

async fn read(stream: &mut (impl tokio::io::AsyncRead + Unpin)) -> Option<Request> {
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

async fn read_header(
    stream: &mut (impl tokio::io::AsyncRead + Unpin),
    bytes: &mut Vec<u8>,
) -> Option<usize> {
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
