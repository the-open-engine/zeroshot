use super::*;

/// Caller-owned stable operation identifier and canonical fingerprint.
pub type SourceOperationIdentity = (SourceOperationId, SourceOperationFingerprint);

/// Optional revision and bounded public URL evidence for a source operation.
pub type SourceRevisionEvidence = (Option<SourceRevisionId>, Vec<SourcePublicUrl>);

/// Expected base and head revisions for a merge.
pub type SourceMergeExpectation = (SourceRevisionId, SourceRevisionId);

/// Integrated revision and bounded public URL evidence for a merge.
pub type SourceMergeEvidence = (SourceRevisionId, Vec<SourcePublicUrl>);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SourceOperation {
    Branch {
        expected_base: SourceRevisionId,
        branch: SourceBranchId,
    },
    Commit {
        expected_head: SourceRevisionId,
        change_digest: SourceContentDigest,
    },
    Push {
        expected_head: SourceRevisionId,
        revision: SourceRevisionId,
    },
    PullRequest {
        expected_base: SourceRevisionId,
        expected_head: SourceRevisionId,
    },
    Checks {
        revision: SourceRevisionId,
    },
    AutoMerge {
        expected_base: SourceRevisionId,
        expected_head: SourceRevisionId,
    },
    MergeQueue {
        expected_base: SourceRevisionId,
        expected_head: SourceRevisionId,
    },
    Merge {
        expected_base: SourceRevisionId,
        expected_head: SourceRevisionId,
    },
}

impl SourceOperation {
    #[must_use]
    pub fn capability(&self) -> SourceCapability {
        match self {
            Self::Branch { .. } => SourceCapability::Branch,
            Self::Commit { .. } => SourceCapability::Commit,
            Self::Push { .. } => SourceCapability::Push,
            Self::PullRequest { .. } => SourceCapability::PullRequest,
            Self::Checks { .. } => SourceCapability::Checks,
            Self::AutoMerge { .. } => SourceCapability::AutoMerge,
            Self::MergeQueue { .. } => SourceCapability::MergeQueue,
            Self::Merge { .. } => SourceCapability::Merge,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceOperationRequest {
    repository: CanonicalRepository,
    credential_handle: SourceCredentialHandleId,
    operation_id: SourceOperationId,
    fingerprint: SourceOperationFingerprint,
    operation: SourceOperation,
}

impl SourceOperationRequest {
    pub fn new(
        repository: CanonicalRepository,
        credential_handle: SourceCredentialHandleId,
        identity: SourceOperationIdentity,
        operation: SourceOperation,
    ) -> Result<Self, SourceContractError> {
        let (operation_id, fingerprint) = identity;
        SourceContractError::checked(Self {
            repository,
            credential_handle,
            operation_id,
            fingerprint,
            operation,
        })
    }

    #[must_use]
    pub fn repository(&self) -> &CanonicalRepository {
        &self.repository
    }

    #[must_use]
    pub fn credential_handle(&self) -> &SourceCredentialHandleId {
        &self.credential_handle
    }

    #[must_use]
    pub fn operation_id(&self) -> &SourceOperationId {
        &self.operation_id
    }

    #[must_use]
    pub fn fingerprint(&self) -> &SourceOperationFingerprint {
        &self.fingerprint
    }

    #[must_use]
    pub fn operation(&self) -> &SourceOperation {
        &self.operation
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "SourceAppliedReceiptWire")]
#[serde(rename_all = "camelCase")]
pub struct SourceAppliedReceipt {
    repository: CanonicalRepository,
    operation_id: SourceOperationId,
    fingerprint: SourceOperationFingerprint,
    capability: SourceCapability,
    revision: Option<SourceRevisionId>,
    public_urls: BoundedVec<SourcePublicUrl>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceAppliedReceiptWire {
    repository: CanonicalRepository,
    operation_id: SourceOperationId,
    fingerprint: SourceOperationFingerprint,
    capability: SourceCapability,
    revision: Option<SourceRevisionId>,
    public_urls: BoundedVec<SourcePublicUrl>,
}

impl TryFrom<SourceAppliedReceiptWire> for SourceAppliedReceipt {
    type Error = SourceContractError;

    fn try_from(wire: SourceAppliedReceiptWire) -> Result<Self, Self::Error> {
        if wire.capability == SourceCapability::Merge {
            return Err(SourceContractError {
                field: "source applied receipt capability",
                reason: "merge requires SourceMergeReceipt".to_owned(),
            });
        }
        SourceContractError::checked(Self {
            repository: wire.repository,
            operation_id: wire.operation_id,
            fingerprint: wire.fingerprint,
            capability: wire.capability,
            revision: wire.revision,
            public_urls: wire.public_urls,
        })
    }
}

impl SourceAppliedReceipt {
    pub fn new(
        repository: CanonicalRepository,
        identity: SourceOperationIdentity,
        capability: SourceCapability,
        evidence: SourceRevisionEvidence,
    ) -> Result<Self, SourceContractError> {
        let (operation_id, fingerprint) = identity;
        let (revision, public_urls) = evidence;
        if capability == SourceCapability::Merge {
            return Err(SourceContractError {
                field: "source applied receipt capability",
                reason: "merge requires SourceMergeReceipt".to_owned(),
            });
        }
        SourceContractError::checked(Self {
            repository,
            operation_id,
            fingerprint,
            capability,
            revision,
            public_urls: BoundedVec::new(public_urls)
                .map_err(|error| SourceContractError::new("public URLs", error))?,
        })
    }

    #[must_use]
    pub fn repository(&self) -> &CanonicalRepository {
        &self.repository
    }

    #[must_use]
    pub fn operation_id(&self) -> &SourceOperationId {
        &self.operation_id
    }

    #[must_use]
    pub fn fingerprint(&self) -> &SourceOperationFingerprint {
        &self.fingerprint
    }

    #[must_use]
    pub fn capability(&self) -> SourceCapability {
        self.capability
    }

    #[must_use]
    pub fn revision(&self) -> Option<&SourceRevisionId> {
        self.revision.as_ref()
    }

    #[must_use]
    pub fn public_urls(&self) -> &[SourcePublicUrl] {
        self.public_urls.as_slice()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMergedState {
    Merged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "SourceMergeReceiptWire")]
#[serde(rename_all = "camelCase")]
pub struct SourceMergeReceipt {
    repository: CanonicalRepository,
    operation_id: SourceOperationId,
    fingerprint: SourceOperationFingerprint,
    expected_base: SourceRevisionId,
    expected_head: SourceRevisionId,
    integrated_revision: SourceRevisionId,
    merged_state: SourceMergedState,
    public_urls: BoundedVec<SourcePublicUrl>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceMergeReceiptWire {
    repository: CanonicalRepository,
    operation_id: SourceOperationId,
    fingerprint: SourceOperationFingerprint,
    expected_base: SourceRevisionId,
    expected_head: SourceRevisionId,
    integrated_revision: SourceRevisionId,
    merged_state: SourceMergedState,
    public_urls: BoundedVec<SourcePublicUrl>,
}

impl TryFrom<SourceMergeReceiptWire> for SourceMergeReceipt {
    type Error = SourceContractError;

    fn try_from(wire: SourceMergeReceiptWire) -> Result<Self, Self::Error> {
        SourceContractError::checked(Self {
            repository: wire.repository,
            operation_id: wire.operation_id,
            fingerprint: wire.fingerprint,
            expected_base: wire.expected_base,
            expected_head: wire.expected_head,
            integrated_revision: wire.integrated_revision,
            merged_state: wire.merged_state,
            public_urls: wire.public_urls,
        })
    }
}

impl SourceMergeReceipt {
    pub fn new(
        repository: CanonicalRepository,
        identity: SourceOperationIdentity,
        expectation: SourceMergeExpectation,
        evidence: SourceMergeEvidence,
    ) -> Result<Self, SourceContractError> {
        let (operation_id, fingerprint) = identity;
        let (expected_base, expected_head) = expectation;
        let (integrated_revision, public_urls) = evidence;
        SourceContractError::checked(Self {
            repository,
            operation_id,
            fingerprint,
            expected_base,
            expected_head,
            integrated_revision,
            merged_state: SourceMergedState::Merged,
            public_urls: BoundedVec::new(public_urls)
                .map_err(|error| SourceContractError::new("public URLs", error))?,
        })
    }

    #[must_use]
    pub fn repository(&self) -> &CanonicalRepository {
        &self.repository
    }

    #[must_use]
    pub fn operation_id(&self) -> &SourceOperationId {
        &self.operation_id
    }

    #[must_use]
    pub fn fingerprint(&self) -> &SourceOperationFingerprint {
        &self.fingerprint
    }

    #[must_use]
    pub fn expected_base(&self) -> &SourceRevisionId {
        &self.expected_base
    }

    #[must_use]
    pub fn expected_head(&self) -> &SourceRevisionId {
        &self.expected_head
    }

    #[must_use]
    pub fn integrated_revision(&self) -> &SourceRevisionId {
        &self.integrated_revision
    }

    #[must_use]
    pub fn merged_state(&self) -> SourceMergedState {
        self.merged_state
    }

    #[must_use]
    pub fn public_urls(&self) -> &[SourcePublicUrl] {
        self.public_urls.as_slice()
    }
}
