use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

pub(super) struct HookScope {
    pub(super) reference: crate::native_v2_contract::ExecutionRef,
    pub(super) auxiliary: bool,
    session_key: String,
    workspace: PathBuf,
    context_budget: AtomicUsize,
    standing: AtomicBool,
}

impl HookScope {
    pub(super) fn new(invocation: &DriverInvocation, workspace: &Path) -> Self {
        Self {
            reference: invocation.node.reference.clone(),
            auxiliary: super::auxiliary_invocation(invocation),
            session_key: super::provider_session_key(invocation),
            workspace: workspace.to_path_buf(),
            context_budget: AtomicUsize::new(native::MAX_CONTEXT_BYTES),
            standing: AtomicBool::new(true),
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
            node_id: self.reference.node.as_str().to_owned(),
            session_key: self.session_key.clone(),
            execution_id: self.reference.execution.get(),
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
            subagent: event["agent_id"].as_str().is_some_and(|id| !id.is_empty()),
            context_budget: self.context_budget.load(Ordering::Acquire),
            standing: self.standing.load(Ordering::Acquire),
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

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicationRequest {
    token: String,
    operation: PublicationOperation,
    params: openengine_cluster_protocol::NetworkSteerParams,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum PublicationOperation {
    Publish,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum BrokerRequest {
    Callback(Request),
    Publication(PublicationRequest),
}

struct ScopeState {
    scope: HookScope,
    live: AtomicBool,
    publication: Arc<tokio::sync::Mutex<()>>,
}
#[derive(Default)]
struct Scopes {
    live: BTreeMap<String, Arc<ScopeState>>,
    retiring: BTreeMap<u64, Vec<Arc<ScopeState>>>,
}

pub(super) struct Broker {
    endpoint: SocketAddr,
    scopes: Arc<Mutex<Scopes>>,
    closed: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Broker {
    pub(super) fn new(source: Arc<super::steering::DeliverySource>) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let endpoint = listener.local_addr()?;
        let scopes = Arc::new(Mutex::new(Scopes::default()));
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
                    let _ = handle_connection(&mut stream, &thread_scopes, &source);
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
        let execution = invocation.node.reference.execution.get();
        let replaced: Vec<_> = scopes
            .live
            .values()
            .filter(|scope| scope.scope.reference.execution.get() == execution)
            .cloned()
            .collect();
        scopes
            .live
            .retain(|_, scope| scope.scope.reference.execution.get() != execution);
        for scope in replaced {
            scope.live.store(false, Ordering::Release);
            scopes.retiring.entry(execution).or_default().push(scope);
        }
        if scopes.live.len() >= MAX_SCOPES {
            return Err(io::Error::other("memory scope budget exhausted"));
        }
        scopes.live.insert(
            token.clone(),
            Arc::new(ScopeState {
                scope: HookScope::new(invocation, workspace),
                live: AtomicBool::new(true),
                publication: Arc::default(),
            }),
        );
        Ok(Capability {
            endpoint: self.endpoint,
            token,
        })
    }

    /// Configures the execution's carrier before its native process starts.
    pub(super) fn configure_carrier(
        &self,
        invocation: &DriverInvocation,
        context_budget: usize,
        standing: bool,
    ) {
        if let Ok(scopes) = self.scopes.lock() {
            for scope in scopes.live.values().filter(|scope| {
                scope.scope.reference.execution.get() == invocation.node.reference.execution.get()
            }) {
                scope
                    .scope
                    .context_budget
                    .store(context_budget, Ordering::Release);
                scope.scope.standing.store(standing, Ordering::Release);
            }
        }
    }

    pub(super) async fn retire(&self, execution: u64) {
        let retired = if let Ok(mut scopes) = self.scopes.lock() {
            let mut retired = scopes.retiring.remove(&execution).unwrap_or_default();
            retired.extend(
                scopes
                    .live
                    .values()
                    .filter(|scope| scope.scope.reference.execution.get() == execution)
                    .cloned(),
            );
            scopes
                .live
                .retain(|_, scope| scope.scope.reference.execution.get() != execution);
            for scope in &retired {
                scope.live.store(false, Ordering::Release);
            }
            retired
        } else {
            Vec::new()
        };
        // Started writes keep their owned fence even when the requesting socket times out.
        for scope in retired {
            let _drained = scope.publication.lock().await;
        }
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

fn handle_connection(
    stream: &mut TcpStream,
    scopes: &Mutex<Scopes>,
    source: &Arc<super::steering::DeliverySource>,
) -> io::Result<()> {
    let bytes = read_frame(stream, native::MAX_CONTEXT_BYTES)?;
    let request: BrokerRequest = serde_json::from_slice(&bytes)?;
    match request {
        BrokerRequest::Callback(request) => {
            if bytes.len() > MAX_EVENT_BYTES {
                return Err(io::Error::other("memory callback frame too large"));
            }
            return_callback(stream, scopes, source, request)
        }
        BrokerRequest::Publication(request) => {
            let result = publish_request(scopes, source, request);
            let bytes = serde_json::to_vec(&result.map_err(|error| error.to_string()))?;
            if bytes.len() > native::MAX_CONTEXT_BYTES {
                return Err(io::Error::other("network publication response too large"));
            }
            write_frame(stream, &bytes)
        }
    }
}

fn return_callback(
    stream: &mut TcpStream,
    scopes: &Mutex<Scopes>,
    source: &super::steering::DeliverySource,
    request: Request,
) -> io::Result<()> {
    let scope = scopes
        .lock()
        .map_err(|_| io::Error::other("memory scope lock unavailable"))?
        .live
        .get(&request.token)
        .cloned();
    let query = scope
        .filter(|scope| scope.live.load(Ordering::Acquire))
        .map(|scope| scope.scope.query(&request.event))
        .filter(|query| !query.tool_name.is_empty());
    let mut delivery = query
        .as_ref()
        .map(|query| source.tool_delivery(query))
        .unwrap_or_default();
    delivery.context = native::bounded_context(delivery.context);
    let returned = return_context(stream, &delivery.context, source);
    if let Some(query) = query {
        if returned {
            source.acknowledged(&query, &delivery);
        } else {
            source.unacknowledged(&query, &delivery);
        }
    }
    Ok(())
}

fn publish_request(
    scopes: &Mutex<Scopes>,
    source: &Arc<super::steering::DeliverySource>,
    request: PublicationRequest,
) -> io::Result<openengine_cluster_protocol::RunSteerResult> {
    let scope = scopes
        .lock()
        .map_err(|_| io::Error::other("network scope unavailable"))?
        .live
        .get(&request.token)
        .cloned()
        .ok_or_else(|| io::Error::other("network publication capability has retired"))?;
    let runtime = source
        .runtime()
        .ok_or_else(|| io::Error::other("steering run unavailable"))?;
    let publisher = source.clone();
    let task = runtime.spawn(async move {
        let _fence = scope.publication.clone().lock_owned().await;
        publisher
            .publish(&scope.scope, &scope.live, request.params)
            .await
    });
    runtime.block_on(async {
        tokio::time::timeout(IO_TIMEOUT, task)
            .await
            .map_err(|_| publication_unknown())?
            .map_err(|_| io::Error::other("network publication unavailable"))?
    })
}

pub(super) fn publish_network(
    path: &Path,
    params: openengine_cluster_protocol::NetworkSteerParams,
) -> io::Result<openengine_cluster_protocol::RunSteerResult> {
    let capability = read_capability(path)?;
    let request = serde_json::to_vec(&PublicationRequest {
        token: capability.token,
        operation: PublicationOperation::Publish,
        params,
    })?;
    if request.len() > native::MAX_CONTEXT_BYTES {
        return Err(io::Error::other("network publication request too large"));
    }
    let mut stream = callback_stream(capability.endpoint)?;
    write_frame(&mut stream, &request).map_err(|_| publication_unknown())?;
    let bytes =
        read_frame(&mut stream, native::MAX_CONTEXT_BYTES).map_err(|_| publication_unknown())?;
    let result: Result<openengine_cluster_protocol::RunSteerResult, String> =
        serde_json::from_slice(&bytes).map_err(|_| publication_unknown())?;
    result.map_err(io::Error::other)
}

pub(super) fn publication_unknown() -> io::Error {
    io::Error::other("network publication outcome is unknown; inspect run vectors before retrying")
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

fn return_context(stream: &mut TcpStream, context: &str, source: &dyn ActiveMemorySource) -> bool {
    if write_frame(stream, context.as_bytes()).is_err() {
        source.unavailable("Active Memory callback context could not be returned");
        return false;
    }
    if context.is_empty() {
        return false;
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
        return false;
    }
    true
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

fn callback_stream(endpoint: SocketAddr) -> io::Result<TcpStream> {
    let stream = TcpStream::connect_timeout(&endpoint, IO_TIMEOUT)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    stream.set_nodelay(true)?;
    Ok(stream)
}

fn query_callback(capability: Capability, event: Value) -> io::Result<(String, TcpStream)> {
    let request = serde_json::to_vec(&Request {
        token: capability.token,
        event,
    })?;
    let mut stream = callback_stream(capability.endpoint)?;
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
