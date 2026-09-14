//! A real smart-HTTP Git endpoint. Only the origin URL is rewritten by the client shim.
//! Authentication is checked at the socket boundary; mutation faults happen after receive-pack.
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use openengine_cluster_testkit::assertions::AssertValue;

#[derive(Default)]
pub(super) struct TransportFaults {
    pub(super) require_refreshed: AtomicBool,
    pub(super) reject_all_credentials: AtomicBool,
    pub(super) fail_fetch_once: AtomicBool,
    pub(super) drop_push_response_once: AtomicBool,
    pub(super) accepted: AtomicUsize,
    pub(super) denied: AtomicUsize,
    pub(super) fetch_failures: AtomicUsize,
    pub(super) receive_packs: AtomicUsize,
    pub(super) dropped_responses: AtomicUsize,
    stop: AtomicBool,
}

pub(super) struct HttpGit {
    pub(super) url: String,
    pub(super) faults: Arc<TransportFaults>,
    worker: Option<JoinHandle<()>>,
}

impl HttpGit {
    pub(super) fn start(remote: &Path) -> Self {
        super::git(remote, &["config", "http.receivepack", "true"]);
        let listener = TcpListener::bind("127.0.0.1:0").assert_value();
        let address = listener.local_addr().assert_value();
        listener.set_nonblocking(true).assert_value();
        let faults = Arc::new(TransportFaults::default());
        let state = faults.clone();
        let root = remote.parent().assert_value().to_owned();
        let worker = thread::spawn(move || serve(listener, root, state));
        Self {
            url: format!("http://{address}/remote.git"),
            faults,
            worker: Some(worker),
        }
    }

    pub(super) fn git_program(&self, repository: &super::TempRepo) -> PathBuf {
        super::git(
            &repository.workspace,
            &["config", "--local", "zeroshotTest.httpOrigin", &self.url],
        );
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/native_v2_delivery/tests/transport_fixture.sh")
    }
}

impl Drop for HttpGit {
    fn drop(&mut self) {
        self.faults.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().assert_value_with("HTTP Git fixture worker");
        }
    }
}

fn serve(listener: TcpListener, root: PathBuf, faults: Arc<TransportFaults>) {
    while !faults.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => serve_connection(stream, &root, &faults),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("HTTP Git listener: {error}"),
        }
    }
}

struct Request {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn serve_connection(mut stream: TcpStream, root: &Path, faults: &TransportFaults) {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .assert_value();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .assert_value();
    let request = read_request(&mut stream);
    if !authenticated(&request, faults) {
        faults.denied.fetch_add(1, Ordering::SeqCst);
        respond(
            &mut stream,
            "401 Unauthorized",
            "WWW-Authenticate: Basic realm=\"git\"\r\n",
            b"",
        );
        return;
    }
    faults.accepted.fetch_add(1, Ordering::SeqCst);
    if request.target.contains("git-upload-pack")
        && faults.fail_fetch_once.swap(false, Ordering::SeqCst)
    {
        faults.fetch_failures.fetch_add(1, Ordering::SeqCst);
        respond(
            &mut stream,
            "503 Service Unavailable",
            "",
            b"injected upload-pack outage",
        );
        return;
    }
    let output = backend(root, &request);
    let receives_pack = request.method == "POST" && request.target.ends_with("/git-receive-pack");
    if receives_pack {
        faults.receive_packs.fetch_add(1, Ordering::SeqCst);
        if faults.drop_push_response_once.swap(false, Ordering::SeqCst) {
            faults.dropped_responses.fetch_add(1, Ordering::SeqCst);
            return;
        }
    }
    relay_cgi(&mut stream, &output);
}

fn authenticated(request: &Request, faults: &TransportFaults) -> bool {
    if faults.reject_all_credentials.load(Ordering::SeqCst) {
        return false;
    }
    let expected = if faults.require_refreshed.load(Ordering::SeqCst) {
        "eC1hY2Nlc3MtdG9rZW46cmVmcmVzaGVkLXRva2Vu"
    } else {
        "eC1hY2Nlc3MtdG9rZW46dGVzdC10b2tlbg=="
    };
    request
        .headers
        .get("authorization")
        .and_then(|value| value.split_once(' '))
        .is_some_and(|(scheme, value)| scheme.eq_ignore_ascii_case("basic") && value == expected)
}

fn read_request(stream: &mut TcpStream) -> Request {
    let mut buffer = Vec::new();
    let header_end = loop {
        if let Some(index) = buffer.windows(4).position(|part| part == b"\r\n\r\n") {
            break index + 4;
        }
        read_more(stream, &mut buffer);
        assert!(
            buffer.len() < 64 * 1024,
            "test HTTP request headers too large"
        );
    };
    let mut storage = [httparse::EMPTY_HEADER; 32];
    let mut parsed = httparse::Request::new(&mut storage);
    assert!(
        parsed
            .parse(&buffer[..header_end])
            .assert_value()
            .is_complete()
    );
    let headers: BTreeMap<String, String> = parsed
        .headers
        .iter()
        .map(|header| {
            (
                header.name.to_ascii_lowercase(),
                String::from_utf8_lossy(header.value).into_owned(),
            )
        })
        .collect();
    let method = parsed.method.assert_value().to_owned();
    let target = parsed.path.assert_value().to_owned();
    assert!(
        !headers.contains_key("transfer-encoding"),
        "small test packs must use Content-Length"
    );
    let length: usize = headers
        .get("content-length")
        .map_or(0, |value| value.parse().assert_value());
    assert!(length <= 1024 * 1024, "test pack unexpectedly large");
    while buffer.len() < header_end + length {
        read_more(stream, &mut buffer);
    }
    Request {
        method,
        target,
        headers,
        body: buffer[header_end..header_end + length].to_vec(),
    }
}

fn read_more(stream: &mut TcpStream, buffer: &mut Vec<u8>) {
    let mut part = [0_u8; 8192];
    let count = stream.read(&mut part).assert_value();
    assert_ne!(count, 0, "Git client closed an incomplete HTTP request");
    buffer.extend_from_slice(&part[..count]);
}

fn backend(root: &Path, request: &Request) -> Vec<u8> {
    let (path, query) = request
        .target
        .split_once('?')
        .unwrap_or((&request.target, ""));
    let mut child = Command::new("/usr/bin/git")
        .arg("http-backend")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("REQUEST_METHOD", &request.method)
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("REMOTE_USER", "delivery-test")
        .env("REMOTE_ADDR", "127.0.0.1")
        .env(
            "CONTENT_TYPE",
            request
                .headers
                .get("content-type")
                .map_or("", String::as_str),
        )
        .env("CONTENT_LENGTH", request.body.len().to_string())
        .env(
            "HTTP_GIT_PROTOCOL",
            request
                .headers
                .get("git-protocol")
                .map_or("", String::as_str),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .assert_value();
    child
        .stdin
        .take()
        .assert_value()
        .write_all(&request.body)
        .assert_value();
    let output = child.wait_with_output().assert_value();
    assert!(
        output.status.success(),
        "git-http-backend: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn relay_cgi(stream: &mut TcpStream, output: &[u8]) {
    let index = output
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .assert_value();
    let headers = String::from_utf8_lossy(&output[..index]);
    let mut status = "200 OK";
    let mut forwarded = String::new();
    for line in headers.split("\r\n") {
        if let Some(value) = line.strip_prefix("Status: ") {
            status = value;
        } else {
            forwarded.push_str(line);
            forwarded.push_str("\r\n");
        }
    }
    respond(stream, status, &forwarded, &output[index + 4..]);
}

fn respond(stream: &mut TcpStream, status: &str, headers: &str, body: &[u8]) {
    write!(
        stream,
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .assert_value();
    stream.write_all(body).assert_value();
}
