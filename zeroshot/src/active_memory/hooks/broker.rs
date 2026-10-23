use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::native_v2_runner::DriverInvocation;

use super::{ActiveMemorySource, HookQuery, native};

const MAX_EVENT_BYTES: usize = 256 * 1024;
const MAX_CAPABILITY_BYTES: u64 = 16 * 1024;
const MAX_SCOPES: usize = 16_384;
const IO_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub(super) struct HookScope {
    node_id: String,
    session_key: String,
    execution_id: u64,
    workspace: PathBuf,
}

impl HookScope {
    pub(super) fn new(invocation: &DriverInvocation, workspace: &Path) -> Self {
        Self {
            node_id: invocation.node.reference.node.as_str().to_owned(),
            session_key: super::provider_session_key(invocation),
            execution_id: invocation.node.reference.execution.get(),
            workspace: workspace.to_path_buf(),
        }
    }

    pub(super) fn query(&self, event: &Value) -> HookQuery {
        let cwd = event["cwd"]
            .as_str()
            .or_else(|| event["workingDirectory"].as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| self.workspace.clone());
        let cwd = if cwd.is_absolute() {
            cwd
        } else {
            self.workspace.join(cwd)
        };
        HookQuery {
            node_id: self.node_id.clone(),
            session_key: self.session_key.clone(),
            execution_id: self.execution_id,
            native_session_id: event["session_id"]
                .as_str()
                .or_else(|| event["sessionId"].as_str())
                .map(str::to_owned),
            tool_name: event["tool_name"]
                .as_str()
                .or_else(|| event["toolName"].as_str())
                .unwrap_or("")
                .to_owned(),
            tool_input: event
                .get("tool_input")
                .or_else(|| event.get("toolArgs"))
                .or_else(|| event.get("input"))
                .cloned()
                .unwrap_or(Value::Null),
            cwd,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Capability {
    pub(super) endpoint: SocketAddr,
    pub(super) token: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    token: String,
    event: Value,
}

pub(super) struct Broker {
    endpoint: SocketAddr,
    scopes: Arc<Mutex<BTreeMap<String, HookScope>>>,
    closed: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Broker {
    pub(super) fn new(source: Arc<dyn ActiveMemorySource>) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let endpoint = listener.local_addr()?;
        let scopes = Arc::new(Mutex::new(BTreeMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let thread_scopes = Arc::clone(&scopes);
        let thread_closed = Arc::clone(&closed);
        let thread = thread::Builder::new()
            .name("active-memory-hooks".to_owned())
            .spawn(move || {
                for connection in listener.incoming() {
                    if thread_closed.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(mut stream) = connection else {
                        break;
                    };
                    if let Err(error) = stream
                        .set_read_timeout(Some(IO_TIMEOUT))
                        .and_then(|()| stream.set_write_timeout(Some(IO_TIMEOUT)))
                        .and_then(|()| stream.set_nodelay(true))
                    {
                        let _ = error;
                        continue;
                    }
                    if let Ok(context) = response(&mut stream, &thread_scopes, source.as_ref()) {
                        return_context(&mut stream, &context, source.as_ref());
                    }
                }
            })?;
        Ok(Self {
            endpoint,
            scopes,
            closed,
            thread: Some(thread),
        })
    }

    pub(super) fn capability(
        &self,
        invocation: &DriverInvocation,
        workspace: &Path,
    ) -> io::Result<Capability> {
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let token = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut scopes = self
            .scopes
            .lock()
            .map_err(|_| io::Error::other("memory scope lock unavailable"))?;
        if scopes.len() >= MAX_SCOPES {
            return Err(io::Error::other("memory scope budget exhausted"));
        }
        scopes.insert(token.clone(), HookScope::new(invocation, workspace));
        Ok(Capability {
            endpoint: self.endpoint,
            token,
        })
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.endpoint, IO_TIMEOUT);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn response(
    stream: &mut TcpStream,
    scopes: &Mutex<BTreeMap<String, HookScope>>,
    source: &dyn ActiveMemorySource,
) -> io::Result<String> {
    let request: Request = serde_json::from_slice(&read_frame(stream, MAX_EVENT_BYTES)?)?;
    let scope = scopes
        .lock()
        .map_err(|_| io::Error::other("memory scope lock unavailable"))?
        .get(&request.token)
        .cloned();
    let Some(scope) = scope else {
        return Ok(String::new());
    };
    let query = scope.query(&request.event);
    if query.tool_name.is_empty() {
        return Ok(String::new());
    }
    Ok(native::bounded_context(source.tool_context(&query)))
}

pub(super) fn run_helper(path: &Path) -> io::Result<()> {
    run_helper_io(path, io::stdin().lock(), io::stdout().lock())
}

pub(super) fn run_helper_io(
    path: &Path,
    input: impl Read,
    mut output: impl Write,
) -> io::Result<()> {
    let (context, callback) = helper_output(path, input).unwrap_or_else(|_| (json!({}), None));
    serde_json::to_writer(&mut output, &context)?;
    output.flush()?;
    if let Some(mut callback) = callback {
        callback.write_all(&[1])?;
    }
    Ok(())
}

fn helper_output(path: &Path, input: impl Read) -> io::Result<(Value, Option<TcpStream>)> {
    let capability = read_capability(path)?;
    let bytes = read_bounded(input, MAX_EVENT_BYTES as u64)?;
    let event: Value = serde_json::from_slice(&bytes)?;
    let name = event["hook_event_name"]
        .as_str()
        .unwrap_or("PreToolUse")
        .to_owned();
    let (context, callback) = query_callback(capability, event)?;
    if context.is_empty() {
        return Ok((json!({}), None));
    }
    Ok((
        json!({"hookSpecificOutput":{"hookEventName":name,"additionalContext":context}}),
        Some(callback),
    ))
}

fn return_context(stream: &mut TcpStream, context: &str, source: &dyn ActiveMemorySource) {
    if write_frame(stream, context.as_bytes()).is_err() {
        source.unavailable("Active Memory callback context could not be returned");
        return;
    }
    if context.is_empty() {
        return;
    }
    // A successful socket write alone does not prove the helper returned its output.
    // The existing read timeout bounds this carrier receipt, including cancelled helpers.
    let mut acknowledgement = [0];
    if DeadlineStream::new(stream)
        .read_exact(&mut acknowledgement)
        .is_err()
        || acknowledgement != [1]
    {
        source.unavailable("Active Memory callback carrier did not acknowledge context");
    }
}

fn read_capability(path: &Path) -> io::Result<Capability> {
    let bytes = read_bounded(std::fs::File::open(path)?, MAX_CAPABILITY_BYTES)?;
    let capability: Capability = serde_json::from_slice(&bytes)?;
    if !capability.endpoint.ip().is_loopback() {
        return Err(io::Error::other("memory callback must be local"));
    }
    Ok(capability)
}

fn read_bounded(reader: impl Read, limit: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::other("memory callback input too large"));
    }
    Ok(bytes)
}

fn query_callback(capability: Capability, event: Value) -> io::Result<(String, TcpStream)> {
    let request = serde_json::to_vec(&Request {
        token: capability.token,
        event,
    })?;
    let mut stream = TcpStream::connect_timeout(&capability.endpoint, IO_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.set_nodelay(true)?;
    write_frame(&mut stream, &request)?;
    let context = String::from_utf8(read_frame(&mut stream, native::MAX_CONTEXT_BYTES)?)
        .map_err(|_| io::Error::other("memory callback was not UTF-8"))?;
    Ok((context, stream))
}

fn read_frame(stream: &mut TcpStream, limit: usize) -> io::Result<Vec<u8>> {
    let mut stream = DeadlineStream::new(stream);
    let mut header = [0; 4];
    stream.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    if length > limit {
        return Err(io::Error::other("memory callback frame too large"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn write_frame(stream: &mut TcpStream, bytes: &[u8]) -> io::Result<()> {
    let mut stream = DeadlineStream::new(stream);
    let length = u32::try_from(bytes.len())
        .map_err(|_| io::Error::other("memory callback frame too large"))?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()
}

/// Socket timeouts apply to individual syscalls. Recompute the remaining absolute
/// budget before every partial read/write so a slow peer cannot extend a frame forever.
struct DeadlineStream<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}

impl<'a> DeadlineStream<'a> {
    fn new(stream: &'a mut TcpStream) -> Self {
        Self {
            stream,
            deadline: Instant::now() + IO_TIMEOUT,
        }
    }

    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "memory callback deadline elapsed")
            })
    }
}

impl Read for DeadlineStream<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}

impl Write for DeadlineStream<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}
