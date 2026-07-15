//! Backend-neutral JSON-RPC dispatcher for Cluster Protocol v1.

pub mod stdio;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    DomainErrorData, GetParams, GetResult, InitializeParams, InitializeResult, JsonRpcError,
    JsonRpcErrorResponse, JsonRpcRequest, JsonRpcSuccess, RequestId, JSONRPC_VERSION,
    PROTOCOL_VERSION,
};
use serde::Serialize;
use serde_json::Value;

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
pub const DOMAIN_ERROR: i64 = -32000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionContext {
    connection_id: String,
}

impl ConnectionContext {
    pub fn new(connection_id: impl Into<String>) -> Self {
        Self {
            connection_id: connection_id.into(),
        }
    }

    pub fn connection_id(&self) -> &str {
        &self.connection_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendError {
    pub code: String,
    pub message: String,
}

impl BackendError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait ClusterBackend: Send + Sync + 'static {
    async fn initialize(
        &self,
        context: &ConnectionContext,
        params: &InitializeParams,
    ) -> Result<InitializeResult, BackendError>;

    async fn get(
        &self,
        context: &ConnectionContext,
        params: &GetParams,
    ) -> Result<GetResult, BackendError>;
}

pub struct Dispatcher<B> {
    backend: B,
    context: ConnectionContext,
}

impl<B> Dispatcher<B>
where
    B: ClusterBackend,
{
    pub fn new(backend: B, context: ConnectionContext) -> Self {
        Self { backend, context }
    }

    pub async fn dispatch_line(&self, request: &str) -> String {
        self.dispatch_bytes(request.as_bytes()).await.response
    }

    pub async fn dispatch_bytes(&self, request: &[u8]) -> DispatchOutcome {
        let request = match std::str::from_utf8(request) {
            Ok(request) => request,
            Err(error) => {
                return error_outcome(
                    ErrorSpec::new(PARSE_ERROR, "Parse error", None, None),
                    format!("invalid UTF-8 JSON-RPC frame: {error}"),
                );
            }
        };
        let value = match serde_json::from_str::<Value>(request) {
            Ok(value) => value,
            Err(error) => {
                return error_outcome(
                    ErrorSpec::new(PARSE_ERROR, "Parse error", None, None),
                    format!("malformed JSON-RPC frame: {error}"),
                );
            }
        };
        self.dispatch_value(value).await
    }

    async fn dispatch_value(&self, value: Value) -> DispatchOutcome {
        let request = match serde_json::from_value::<JsonRpcRequest>(value) {
            Ok(request) => request,
            Err(error) => {
                return error_outcome(
                    ErrorSpec::new(INVALID_REQUEST, "Invalid Request", None, None),
                    format!("invalid JSON-RPC request envelope: {error}"),
                );
            }
        };
        if request.jsonrpc != JSONRPC_VERSION {
            return error_outcome(
                ErrorSpec::new(INVALID_REQUEST, "Invalid Request", None, None),
                "JSON-RPC request must use jsonrpc 2.0".to_owned(),
            );
        }
        let id = request.id;
        if !request.params.is_object() {
            return error_outcome(
                ErrorSpec::new(INVALID_PARAMS, "Invalid params", Some(id), None),
                "JSON-RPC method params must be a named object".to_owned(),
            );
        }

        match request.method.as_str() {
            "initialize" => self.dispatch_initialize(id, request.params).await,
            "get" => self.dispatch_get(id, request.params).await,
            _ => error_outcome(
                ErrorSpec::new(METHOD_NOT_FOUND, "Method not found", Some(id), None),
                format!("unknown JSON-RPC method: {}", request.method),
            ),
        }
    }

    async fn dispatch_initialize(&self, id: RequestId, params: Value) -> DispatchOutcome {
        let params = match serde_json::from_value::<InitializeParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return error_outcome(
                    ErrorSpec::new(INVALID_PARAMS, "Invalid params", Some(id), None),
                    format!("invalid initialize params: {error}"),
                );
            }
        };
        if params.protocol_version != PROTOCOL_VERSION {
            return error_outcome(
                ErrorSpec::new(
                    DOMAIN_ERROR,
                    "Unsupported protocol version",
                    Some(id),
                    Some(DomainErrorData::new("UNSUPPORTED_PROTOCOL_VERSION")),
                ),
                format!(
                    "unsupported protocol version {}; expected {PROTOCOL_VERSION}",
                    params.protocol_version
                ),
            );
        }
        match self.backend.initialize(&self.context, &params).await {
            Ok(result) => success_outcome(id, result),
            Err(error) => backend_error_outcome(id, error),
        }
    }

    async fn dispatch_get(&self, id: RequestId, params: Value) -> DispatchOutcome {
        let params = match serde_json::from_value::<GetParams>(params) {
            Ok(params) => params,
            Err(error) => {
                return error_outcome(
                    ErrorSpec::new(INVALID_PARAMS, "Invalid params", Some(id), None),
                    format!("invalid get params: {error}"),
                );
            }
        };
        match self.backend.get(&self.context, &params).await {
            Ok(result) => success_outcome(id, result),
            Err(error) => backend_error_outcome(id, error),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchOutcome {
    pub response: String,
    pub diagnostic: Option<String>,
}

fn success_outcome<T>(id: RequestId, result: T) -> DispatchOutcome
where
    T: Serialize,
{
    let response = JsonRpcSuccess {
        jsonrpc: JSONRPC_VERSION.to_owned(),
        id,
        result,
    };
    DispatchOutcome {
        response: serde_json::to_string(&response)
            .expect("JSON-RPC success response types must serialize"),
        diagnostic: None,
    }
}

fn backend_error_outcome(id: RequestId, error: BackendError) -> DispatchOutcome {
    let diagnostic = format!("backend failure {}: {}", error.code, error.message);
    error_outcome(
        ErrorSpec::new(
            INTERNAL_ERROR,
            "Internal error",
            Some(id),
            Some(DomainErrorData::new(error.code)),
        ),
        diagnostic,
    )
}

struct ErrorSpec {
    code: i64,
    message: &'static str,
    id: Option<RequestId>,
    data: Option<DomainErrorData>,
}

impl ErrorSpec {
    fn new(
        code: i64,
        message: &'static str,
        id: Option<RequestId>,
        data: Option<DomainErrorData>,
    ) -> Self {
        Self {
            code,
            message,
            id,
            data,
        }
    }
}

fn error_outcome(spec: ErrorSpec, diagnostic: String) -> DispatchOutcome {
    let response = JsonRpcErrorResponse {
        jsonrpc: JSONRPC_VERSION.to_owned(),
        id: spec.id,
        error: JsonRpcError {
            code: spec.code,
            message: spec.message.to_owned(),
            data: spec.data,
        },
    };
    DispatchOutcome {
        response: serde_json::to_string(&response)
            .expect("JSON-RPC error response types must serialize"),
        diagnostic: Some(diagnostic),
    }
}
