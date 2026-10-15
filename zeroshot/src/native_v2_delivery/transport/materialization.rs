//! An invocation-scoped Git checkout capability minted only by the workspace owner.
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use crate::source_code_provider::{
    SourceContentDigest, SourceDeliveryCredential, SourceDeliveryError, SourceGitCheckout,
    SourceMaterializationError, SourceMaterializationTarget,
};
use super::{CheckoutRequest, GitTransport};

pub(crate) struct GitMaterializationTarget<'a> {
    transport: &'a GitTransport,
    remote: &'a str,
    branch: &'a str,
    revision: &'a str,
    credential: SourceDeliveryCredential<'a>,
    deadline: tokio::time::Instant,
    available: AtomicBool,
    failure: Mutex<Option<SourceDeliveryError>>,
}

impl<'a> GitMaterializationTarget<'a> {
    pub(crate) fn new(
        transport: &'a GitTransport,
        source: (&'a str, &'a str, &'a str),
        credential: SourceDeliveryCredential<'a>,
        deadline: tokio::time::Instant,
    ) -> Self {
        Self {
            transport,
            remote: source.0,
            branch: source.1,
            revision: source.2,
            credential,
            deadline,
            available: AtomicBool::new(true),
            failure: Mutex::new(None),
        }
    }

    pub(crate) fn take_failure(&self) -> Option<SourceDeliveryError> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn failed(&self, error: SourceDeliveryError) -> SourceMaterializationError {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(error);
        SourceMaterializationError
    }
}

#[async_trait]
impl SourceMaterializationTarget for GitMaterializationTarget<'_> {
    fn is_available(&self) -> bool {
        self.available.load(Ordering::SeqCst)
    }

    fn write_file(&self, _name: &str, _contents: &[u8]) -> Result<(), SourceMaterializationError> {
        Err(SourceMaterializationError)
    }

    async fn checkout_git(
        &self,
        request: SourceGitCheckout<'_>,
    ) -> Result<SourceContentDigest, SourceMaterializationError> {
        if !self.available.swap(false, Ordering::SeqCst)
            || request.remote != self.remote
            || request.branch != self.branch
            || request.revision.as_str() != self.revision
        {
            return Err(self.failed(SourceDeliveryError::identity(
                "materialization request changed the admitted Git source",
            )));
        }
        self.transport
            .checkout_before(
                CheckoutRequest {
                    branch: self.branch,
                    revision: self.revision,
                },
                self.credential,
                self.deadline,
            )
            .await
            .map_err(|error| self.failed(error))?;
        // Git's exact commit binds the entire tree and commit metadata. Rehash that content address
        // into the domain's fixed SHA-256 receipt digest without including locator or credentials.
        let mut digest = Sha256::new();
        digest.update(b"openengine.git-source/v1\n");
        digest.update(self.revision.as_bytes());
        SourceContentDigest::new(format!("{:x}", digest.finalize()))
            .map_err(|_| SourceMaterializationError)
    }
}
