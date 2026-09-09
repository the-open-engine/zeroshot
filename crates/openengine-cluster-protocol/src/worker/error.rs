use thiserror::Error;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum WorkerContractError {
    #[error("unsupported worker protocol version/profile binding")]
    UnsupportedProtocolBinding,
    #[error("invalid worker binding {0}")]
    InvalidProtocolBindingComponent(&'static str),
    #[error("invalid opaque registry handle")]
    InvalidOpaqueHandle,
    #[error("{0} must not be empty")]
    Empty(&'static str),
    #[error("{0} must not contain duplicates")]
    Duplicate(&'static str),
    #[error("worker errors must contain timeout, crash, malformed, and refusal")]
    IncompleteRuntimeErrors,
    #[error("openengine.worker.builtin/v1 descriptors must not declare credential requirements")]
    InvalidBuiltinBinding,
    #[error("worker error code and failure reason are inconsistent")]
    InvalidFailurePair,
}
