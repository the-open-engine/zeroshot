use std::io;
use std::process::Output;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::path::PathBuf;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::Notify;

pub const API_KEY: &str = "export-fixture-secret";
pub const ORGANIZATION: &str = "00000000-0000-4000-8000-000000000001";

#[derive(Clone, Copy)]
pub enum Reply {
    Length,
    Chunked(&'static str),
    RejectedChunked(&'static str),
    Status,
    Redirect,
    WrongType,
    Partial,
    TruncatedLength,
    TruncatedChunks(&'static str),
    ExporterFailure(&'static str),
    LengthAndTransferEncoding(&'static str),
    CloseDelimited,
    Race,
    #[cfg(unix)]
    StallHeaders,
    #[cfg(unix)]
    StallBody,
}

#[derive(Debug)]
pub struct Request {
    pub path: String,
    pub headers: String,
    pub body: Value,
}

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub output: PathBuf,
    pub requests: Arc<Mutex<Vec<Request>>>,
    pub started: Arc<Notify>,
    pub archive: Vec<u8>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Fixture {
    pub async fn new(reply: Reply, capability: Value) -> Self {
        let root = tempfile::tempdir().unwrap();
        let output_directory = root.path().join("downloads");
        std::fs::create_dir(&output_directory).unwrap();
        let output = output_directory.join("workspace.tar");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let discovery = discovery(&origin, capability);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let started = Arc::new(Notify::new());
        let notified = started.clone();
        let destination = output.clone();
        let archive = archive();
        let contents = archive.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let request = read_request(&mut socket).await;
                let export = request.path == "/native-v2/runs/run-export/export-workspace";
                let discovery_request = request.path == "/.well-known/zeroshot-native-v2";
                assert!(
                    export || discovery_request,
                    "unexpected operation: {}",
                    request.path
                );
                if export {
                    assert!(request.headers.starts_with("POST "));
                    assert!(
                        request
                            .headers
                            .contains(&format!("authorization: Bearer {API_KEY}\r\n"))
                    );
                    assert!(
                        request
                            .headers
                            .contains(&format!("x-zeroshot-organization: {ORGANIZATION}\r\n"))
                    );
                    assert!(request.headers.contains("accept: application/x-tar\r\n"));
                    assert!(request.headers.contains("cache-control: no-store\r\n"));
                }
                captured.lock().unwrap().push(request);
                if discovery_request {
                    respond(
                        &mut socket,
                        "200 OK",
                        "application/json",
                        discovery.as_bytes(),
                    )
                    .await
                    .unwrap();
                } else {
                    if matches!(reply, Reply::Race) {
                        std::fs::write(&destination, b"racing destination").unwrap();
                    }
                    if let Err(error) = export_reply(&mut socket, reply, &contents).await {
                        // These responses are rejected at their headers, so the CLI may close
                        // the connection before the fixture finishes sending the body.
                        assert!(
                            matches!(
                                reply,
                                Reply::Status
                                    | Reply::Redirect
                                    | Reply::WrongType
                                    | Reply::Partial
                                    | Reply::LengthAndTransferEncoding(_)
                                    | Reply::RejectedChunked(_)
                                    | Reply::CloseDelimited
                            ) && matches!(
                                error.kind(),
                                io::ErrorKind::BrokenPipe
                                    | io::ErrorKind::ConnectionReset
                                    | io::ErrorKind::ConnectionAborted
                            ),
                            "unexpected export response failure: {error}"
                        );
                    }
                    notified.notify_one();
                    #[cfg(unix)]
                    if matches!(reply, Reply::StallHeaders | Reply::StallBody) {
                        let mut buf = [0; 1];
                        // Cancellation must drop the actual response/connection.
                        assert_eq!(socket.read(&mut buf).await.unwrap(), 0);
                    }
                }
            }
        });
        let fixture = Self {
            root,
            output,
            requests,
            started,
            archive,
            task,
        };
        let added = fixture
            .command()
            .args(["target", "add", "fixture", "--url", &origin])
            .output()
            .await
            .unwrap();
        assert!(
            added.status.success(),
            "{}",
            String::from_utf8_lossy(&added.stderr)
        );
        fixture.requests.lock().unwrap().clear();
        fixture
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_zeroshot"));
        command
            .kill_on_drop(true)
            .env("ZEROSHOT_CONFIG_DIR", self.root.path().join("config"))
            .env("ZEROSHOT_API_KEY", API_KEY)
            .env("ZEROSHOT_ORGANIZATION", ORGANIZATION)
            .env_remove("ZEROSHOT_ERROR_FORMAT")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost");
        command
    }

    pub fn export_command(&self) -> Command {
        let mut command = self.command();
        command
            .args([
                "export-workspace",
                "run-export",
                "--target",
                "fixture",
                "--output",
            ])
            .arg(&self.output);
        command
    }

    pub async fn export(&self, checkpoint: bool) -> Output {
        let mut command = self.export_command();
        if checkpoint {
            command.args(["--from-checkpoint", "entry-1"]);
        }
        tokio::time::timeout(Duration::from_secs(15), command.output())
            .await
            .unwrap()
            .unwrap()
    }

    pub fn assert_clean(&self) {
        assert!(
            !self.task.is_finished(),
            "HTTP fixture stopped unexpectedly"
        );
        let paths = std::fs::read_dir(self.output.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert!(
            !paths
                .iter()
                .any(|name| name.to_string_lossy().starts_with(".zeroshot-export-")),
            "left temporary files: {paths:?}"
        );
    }
}

pub fn capability() -> Value {
    json!({"kind":"openengine.workspace-export/v1", "route_template":"/native-v2/runs/{run_id}/export-workspace"})
}

fn discovery(origin: &str, capability: Value) -> String {
    json!({
        "kind":"zeroshot.native-v2-target/v2", "authentication":"hosted_oauth",
        "runPath":"/native-v2/run", "sessionPath":"/native-v2/oecp-session", "oecpPath":"/native-v2/oecp", "audience":"controller",
        "oauth": {
            "metadataUrl": format!("{origin}/oauth/metadata"), "deviceAuthorizationEndpoint":format!("{origin}/oauth/device"),
            "tokenEndpoint":format!("{origin}/oauth/token"), "revocationEndpoint":format!("{origin}/oauth/revoke"),
            "clientId":"zeroshot-cli", "deviceGrantType":"urn:ietf:params:oauth:grant-type:device_code", "deviceExchangeFields":["device_token","device_label"]
        },
        "loginSession":{"routeTemplate":"/session", "method":"GET", "cachePolicy":"no-store"},
        "extensions": {
            "api_key":{"kind":"zeroshot.api-key-authentication/v1", "header":"Authorization", "scheme":"Bearer", "organization_header":"X-Zeroshot-Organization", "identity_path":"/machine", "delegations_path":"/delegations"},
            "hosted_runs":{"kind":"zeroshot.hosted-runs/v1", "base_url":origin, "route_templates": {
                "list":"/native-v2/runs", "status":"/native-v2/runs/{run_id}", "watch":"/native-v2/runs/{run_id}/watch{?from_cursor}",
                "logs":"/native-v2/runs/{run_id}/logs{?from_cursor,execution}", "force":"/native-v2/runs/{run_id}/force"
            }},
            "workspace_export":capability
        }
    }).to_string()
}

async fn read_request(socket: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let mut buf = [0; 4096];
    let (end, length) = loop {
        let count = socket.read(&mut buf).await.unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&buf[..count]);
        assert!(bytes.len() < 64 * 1024);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(|value| value.parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            break (end + 4, length);
        }
    };
    while bytes.len() < end + length {
        let count = socket.read(&mut buf).await.unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&buf[..count]);
    }
    let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let path = headers.split_whitespace().nth(1).unwrap().to_owned();
    let body = if length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[end..end + length]).unwrap()
    };
    Request {
        path,
        headers,
        body,
    }
}

async fn respond(
    socket: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nContent-Disposition: attachment; filename=untrusted.tar\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(headers.as_bytes()).await?;
    socket.write_all(body).await?;
    socket.shutdown().await
}

async fn export_reply(socket: &mut TcpStream, reply: Reply, archive: &[u8]) -> io::Result<()> {
    match reply {
        Reply::Length | Reply::Race => {
            respond(socket, "200 OK", "application/x-tar", archive).await?
        }
        Reply::Status => respond(
            socket,
            "500 Internal Server Error",
            "application/json",
            br#"{"code":"export_failed","message":"exporter unavailable export-fixture-secret"}"#,
        )
        .await?,
        Reply::Redirect => {
            socket.write_all(b"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:1/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
        }
        Reply::WrongType => respond(socket, "200 OK", "application/json", archive).await?,
        Reply::Partial => {
            respond(socket, "206 Partial Content", "application/x-tar", archive).await?
        }
        Reply::TruncatedLength | Reply::LengthAndTransferEncoding(_) => {
            let encoding = match reply {
                Reply::LengthAndTransferEncoding(encoding) => {
                    format!("Transfer-Encoding: {encoding}\r\n")
                }
                _ => String::new(),
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\n{encoding}Content-Length: {}\r\nConnection: close\r\n\r\n", archive.len() + 100).as_bytes()).await?;
            socket.write_all(archive).await?;
        }
        Reply::Chunked(encoding)
        | Reply::RejectedChunked(encoding)
        | Reply::TruncatedChunks(encoding)
        | Reply::ExporterFailure(encoding) => {
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nTransfer-Encoding: {encoding}\r\nConnection: close\r\n\r\n").as_bytes()).await?;
            write_chunked_body(socket, reply, archive).await?;
        }
        Reply::CloseDelimited => {
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nConnection: close\r\n\r\n").await?;
            socket.write_all(archive).await?;
        }
        #[cfg(unix)]
        Reply::StallHeaders => {}
        #[cfg(unix)]
        Reply::StallBody => {
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/x-tar\r\nTransfer-Encoding: chunked\r\n\r\n").await?;
            write_chunked_body(socket, reply, archive).await?;
        }
    }
    Ok(())
}

async fn write_chunked_body(
    socket: &mut TcpStream,
    reply: Reply,
    archive: &[u8],
) -> io::Result<()> {
    let length = if matches!(reply, Reply::TruncatedChunks(_)) {
        archive.len() + 100
    } else {
        archive.len()
    };
    socket
        .write_all(format!("{length:x}\r\n").as_bytes())
        .await?;
    socket.write_all(archive).await?;
    if !matches!(reply, Reply::TruncatedChunks(_)) {
        socket.write_all(b"\r\n").await?;
    }
    if matches!(reply, Reply::Chunked(_) | Reply::RejectedChunked(_)) {
        socket.write_all(b"0\r\n\r\n").await?;
    }
    Ok(())
}

fn archive() -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    let bytes = b"retained workspace contents\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    tar.append_data(&mut header, "workspace.txt", &bytes[..])
        .unwrap();
    tar.into_inner().unwrap()
}
