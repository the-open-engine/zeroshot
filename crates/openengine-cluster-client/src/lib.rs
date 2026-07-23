//! Typed transport-neutral Cluster Protocol client.

pub mod watch;
pub use watch::*;

use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use openengine_cluster_protocol::{
    ApplyParams, ApplyResult, GetParams, GetResult, InitializeParams, InitializeResult,
    JsonRpcError, JsonRpcErrorResponse, JsonRpcRequest, JsonRpcSuccess, PlanParams, PlanResult,
    RequestId, StopParams, StopResult, UpdateParams, UpdateResult, JSON_RPC_VERSION,
    PROTOCOL_VERSION,
};
use openengine_cluster_server::{ClusterBackend, Dispatcher};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("transport I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("transport protocol failed: {0}")]
    Protocol(String),
}

#[async_trait]
pub trait JsonRpcTransport: Send + Sync {
    async fn request(&self, request: String) -> Result<String, TransportError>;
}

pub struct InProcessTransport<B> {
    dispatcher: Dispatcher<B>,
}

impl<B> InProcessTransport<B>
where
    B: ClusterBackend,
{
    #[must_use]
    pub const fn new(dispatcher: Dispatcher<B>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait]
impl<B> JsonRpcTransport for InProcessTransport<B>
where
    B: ClusterBackend,
{
    async fn request(&self, request: String) -> Result<String, TransportError> {
        Ok(self.dispatcher.dispatch(&request).await)
    }
}

struct NdjsonIo<R, W> {
    reader: BufReader<R>,
    writer: W,
}

pub struct NdjsonTransport<R, W> {
    io: Mutex<NdjsonIo<R, W>>,
}

impl<R, W> NdjsonTransport<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    #[must_use]
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            io: Mutex::new(NdjsonIo {
                reader: BufReader::new(reader),
                writer,
            }),
        }
    }
}

#[async_trait]
impl<R, W> JsonRpcTransport for NdjsonTransport<R, W>
where
    R: AsyncRead + Send + Unpin,
    W: AsyncWrite + Send + Unpin,
{
    async fn request(&self, request: String) -> Result<String, TransportError> {
        let mut io = self.io.lock().await;
        io.writer.write_all(request.as_bytes()).await?;
        io.writer.write_all(b"\n").await?;
        io.writer.flush().await?;

        let mut response = String::new();
        if io.reader.read_line(&mut response).await? == 0 {
            return Err(TransportError::Protocol(
                "server closed stdout before returning a response".to_owned(),
            ));
        }
        while response.ends_with(['\n', '\r']) {
            response.pop();
        }
        Ok(response)
    }
}

pub struct ClusterClient<T> {
    transport: T,
    next_id: AtomicI64,
}

impl<T> ClusterClient<T>
where
    T: JsonRpcTransport,
{
    #[must_use]
    pub const fn new(transport: T) -> Self {
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
        let protocol_version = protocol_version.into();
        let result: InitializeResult = self
            .call(
                "initialize",
                InitializeParams {
                    protocol_version: protocol_version.clone(),
                },
            )
            .await?;
        if result.protocol_version != protocol_version {
            return Err(ClientError::InvalidResponse(format!(
                "protocol version mismatch: requested {protocol_version}, received {}",
                result.protocol_version
            )));
        }
        result
            .validate_protocol_version()
            .map_err(|error| ClientError::InvalidResponse(error.to_string()))?;
        Ok(result)
    }

    pub async fn plan(&self, params: PlanParams) -> Result<PlanResult, ClientError> {
        self.call("plan", params).await
    }

    pub async fn apply(&self, params: ApplyParams) -> Result<ApplyResult, ClientError> {
        self.call("apply", params).await
    }

    pub async fn get(&self, params: GetParams) -> Result<GetResult, ClientError> {
        self.call("get", params).await
    }

    pub async fn update(&self, params: UpdateParams) -> Result<UpdateResult, ClientError> {
        self.call("update", params).await
    }

    pub async fn stop(&self, params: StopParams) -> Result<StopResult, ClientError> {
        self.call("stop", params).await
    }

    async fn call<P, R>(&self, method: &str, params: P) -> Result<R, ClientError>
    where
        P: Serialize + Send,
        R: DeserializeOwned,
    {
        let id = RequestId::Integer(self.next_id.fetch_add(1, Ordering::Relaxed));
        let request = serde_json::to_string(&JsonRpcRequest {
            jsonrpc: JSON_RPC_VERSION.to_owned(),
            id: id.clone(),
            method: method.to_owned(),
            params,
        })?;
        let response = self.transport.request(request).await?;
        let value: Value = serde_json::from_str(&response)
            .map_err(|error| ClientError::InvalidResponse(error.to_string()))?;

        if value.get("error").is_some() {
            let response: JsonRpcErrorResponse = serde_json::from_value(value)
                .map_err(|error| ClientError::InvalidResponse(error.to_string()))?;
            validate_response_identity(&response.jsonrpc, response.id.as_ref(), &id)?;
            return Err(ClientError::Rpc(response.error));
        }

        let response: JsonRpcSuccess<R> = serde_json::from_value(value)
            .map_err(|error| ClientError::InvalidResponse(error.to_string()))?;
        validate_response_identity(&response.jsonrpc, Some(&response.id), &id)?;
        Ok(response.result)
    }
}

fn validate_response_identity(
    jsonrpc: &str,
    actual_id: Option<&RequestId>,
    expected_id: &RequestId,
) -> Result<(), ClientError> {
    if jsonrpc != JSON_RPC_VERSION {
        return Err(ClientError::InvalidResponse(format!(
            "expected jsonrpc {JSON_RPC_VERSION}, received {jsonrpc}"
        )));
    }
    if actual_id != Some(expected_id) {
        return Err(ClientError::InvalidResponse(format!(
            "response id mismatch: expected {expected_id:?}, received {actual_id:?}"
        )));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("request serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("server returned JSON-RPC error {0:?}")]
    Rpc(JsonRpcError),
    #[error("invalid JSON-RPC response: {0}")]
    InvalidResponse(String),
    #[error(transparent)]
    Backend(#[from] openengine_cluster_server::BackendError),
}
