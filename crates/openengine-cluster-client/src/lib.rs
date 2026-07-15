//! Typed Cluster Protocol client and transport boundaries.

use async_trait::async_trait;
use openengine_cluster_protocol::{
    DomainErrorData, GetParams, GetResult, InitializeParams, InitializeResult,
    JsonRpcErrorResponse, RequestId, JSONRPC_VERSION, PROTOCOL_VERSION,
};
use openengine_cluster_server::{ClusterBackend, Dispatcher};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicI64, Ordering};
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("transport I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("transport closed before a JSON-RPC response was received")]
    Closed,
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("invalid JSON-RPC response: {0}")]
    InvalidResponse(String),
    #[error("JSON-RPC error {code}: {message}")]
    Rpc {
        code: i64,
        message: String,
        data: Option<DomainErrorData>,
    },
}

#[async_trait]
pub trait JsonRpcTransport: Send + Sync {
    async fn request(&self, request: Value) -> Result<Value, TransportError>;
}

pub struct ClusterClient<T> {
    transport: T,
    next_id: AtomicI64,
}

impl<T> ClusterClient<T>
where
    T: JsonRpcTransport,
{
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            next_id: AtomicI64::new(1),
        }
    }

    pub async fn initialize(&self) -> Result<InitializeResult, ClientError> {
        self.initialize_with_version(PROTOCOL_VERSION).await
    }

    pub async fn initialize_with_version(
        &self,
        protocol_version: impl Into<String> + Send,
    ) -> Result<InitializeResult, ClientError> {
        self.call(
            "initialize",
            InitializeParams {
                protocol_version: protocol_version.into(),
            },
        )
        .await
    }

    pub async fn get(&self) -> Result<GetResult, ClientError> {
        self.call("get", GetParams {}).await
    }

    async fn call<P, R>(&self, method: &str, params: P) -> Result<R, ClientError>
    where
        P: serde::Serialize + Send,
        R: DeserializeOwned,
    {
        let id = RequestId::Integer(self.next_id.fetch_add(1, Ordering::Relaxed));
        let request = json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id,
            "method": method,
            "params": params,
        });
        let response = self.transport.request(request).await?;
        if response.get("jsonrpc").and_then(Value::as_str) != Some(JSONRPC_VERSION) {
            return Err(ClientError::InvalidResponse(
                "missing jsonrpc 2.0 marker".to_owned(),
            ));
        }
        let response_id = response
            .get("id")
            .cloned()
            .ok_or_else(|| ClientError::InvalidResponse("missing response id".to_owned()))?;
        let response_id = serde_json::from_value::<RequestId>(response_id).map_err(|error| {
            ClientError::InvalidResponse(format!("invalid response id: {error}"))
        })?;
        if response_id != id {
            return Err(ClientError::InvalidResponse(format!(
                "response id {response_id:?} does not match request id {id:?}"
            )));
        }
        if response.get("error").is_some() {
            let error = serde_json::from_value::<JsonRpcErrorResponse>(response)
                .map_err(|error| ClientError::InvalidResponse(error.to_string()))?
                .error;
            return Err(ClientError::Rpc {
                code: error.code,
                message: error.message,
                data: error.data,
            });
        }
        let result = response
            .get("result")
            .cloned()
            .ok_or_else(|| ClientError::InvalidResponse("missing result or error".to_owned()))?;
        serde_json::from_value(result)
            .map_err(|error| ClientError::InvalidResponse(format!("invalid result: {error}")))
    }
}

pub struct InProcessTransport<B> {
    dispatcher: Dispatcher<B>,
}

impl<B> InProcessTransport<B>
where
    B: ClusterBackend,
{
    pub fn new(dispatcher: Dispatcher<B>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait]
impl<B> JsonRpcTransport for InProcessTransport<B>
where
    B: ClusterBackend,
{
    async fn request(&self, request: Value) -> Result<Value, TransportError> {
        let serialized = serde_json::to_vec(&request)?;
        let response = self.dispatcher.dispatch_bytes(&serialized).await.response;
        Ok(serde_json::from_str(&response)?)
    }
}

pub struct NdjsonTransport<R, W> {
    io: Mutex<NdjsonIo<R, W>>,
}

struct NdjsonIo<R, W> {
    reader: R,
    writer: W,
}

impl<R, W> NdjsonTransport<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            io: Mutex::new(NdjsonIo { reader, writer }),
        }
    }
}

#[async_trait]
impl<R, W> JsonRpcTransport for NdjsonTransport<R, W>
where
    R: AsyncBufRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin,
{
    async fn request(&self, request: Value) -> Result<Value, TransportError> {
        let mut io = self.io.lock().await;
        let mut frame = serde_json::to_vec(&request)?;
        frame.push(b'\n');
        io.writer.write_all(&frame).await?;
        io.writer.flush().await?;

        let mut response = String::new();
        if io.reader.read_line(&mut response).await? == 0 {
            return Err(TransportError::Closed);
        }
        let response = response.trim_end_matches(['\r', '\n']);
        Ok(serde_json::from_str(response)?)
    }
}
