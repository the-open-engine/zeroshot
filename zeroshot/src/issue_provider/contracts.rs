use super::*;

/// Account and canonical issue identity returned by resolution.
pub type IssueResolutionIdentity = (IssueAccountId, IssueId);

/// Closed state and bounded public URL evidence returned by resolution.
pub type IssueResolutionEvidence = (IssueState, Vec<IssuePublicUrl>);

/// Account and opaque credential handle used to resolve an issue.
pub type IssueResolveAccess = (IssueAccountId, IssueCredentialHandleId);

/// Caller-owned stable close-operation identifier and canonical fingerprint.
pub type IssueOperationIdentity = (IssueOperationId, IssueOperationFingerprint);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueState {
    Open,
    Closed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "ResolvedIssueWire")]
#[serde(rename_all = "camelCase")]
pub struct ResolvedIssue {
    provider: IssueProviderRef,
    profile: IssueProfileId,
    account: IssueAccountId,
    issue: IssueId,
    state: IssueState,
    public_urls: BoundedVec<IssuePublicUrl>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolvedIssueWire {
    provider: IssueProviderRef,
    profile: IssueProfileId,
    account: IssueAccountId,
    issue: IssueId,
    state: IssueState,
    public_urls: BoundedVec<IssuePublicUrl>,
}

impl TryFrom<ResolvedIssueWire> for ResolvedIssue {
    type Error = IssueContractError;

    fn try_from(wire: ResolvedIssueWire) -> Result<Self, Self::Error> {
        IssueContractError::checked(Self {
            provider: wire.provider,
            profile: wire.profile,
            account: wire.account,
            issue: wire.issue,
            state: wire.state,
            public_urls: wire.public_urls,
        })
    }
}

impl ResolvedIssue {
    pub fn new(
        provider: IssueProviderRef,
        profile: IssueProfileId,
        identity: IssueResolutionIdentity,
        evidence: IssueResolutionEvidence,
    ) -> Result<Self, IssueContractError> {
        let (account, issue) = identity;
        let (state, public_urls) = evidence;
        IssueContractError::checked(Self {
            provider,
            profile,
            account,
            issue,
            state,
            public_urls: BoundedVec::new(public_urls)
                .map_err(|error| IssueContractError::new("public URLs", error))?,
        })
    }

    #[must_use]
    pub fn provider(&self) -> &IssueProviderRef {
        &self.provider
    }

    #[must_use]
    pub fn profile(&self) -> &IssueProfileId {
        &self.profile
    }

    #[must_use]
    pub fn account(&self) -> &IssueAccountId {
        &self.account
    }

    #[must_use]
    pub fn issue(&self) -> &IssueId {
        &self.issue
    }

    #[must_use]
    pub fn state(&self) -> IssueState {
        self.state
    }

    #[must_use]
    pub fn public_urls(&self) -> &[IssuePublicUrl] {
        self.public_urls.as_slice()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueResolveRequest {
    provider: IssueProviderRef,
    profile: IssueProfileId,
    account: IssueAccountId,
    credential_handle: IssueCredentialHandleId,
    reference: IssueReference,
}

impl IssueResolveRequest {
    pub fn new(
        provider: IssueProviderRef,
        profile: IssueProfileId,
        access: IssueResolveAccess,
        reference: IssueReference,
    ) -> Result<Self, IssueContractError> {
        let (account, credential_handle) = access;
        IssueContractError::checked(Self {
            provider,
            profile,
            account,
            credential_handle,
            reference,
        })
    }

    #[must_use]
    pub fn provider(&self) -> &IssueProviderRef {
        &self.provider
    }

    #[must_use]
    pub fn profile(&self) -> &IssueProfileId {
        &self.profile
    }

    #[must_use]
    pub fn account(&self) -> &IssueAccountId {
        &self.account
    }

    #[must_use]
    pub fn credential_handle(&self) -> &IssueCredentialHandleId {
        &self.credential_handle
    }

    #[must_use]
    pub fn reference(&self) -> &IssueReference {
        &self.reference
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "IssueCloseRequestWire")]
#[serde(rename_all = "camelCase")]
pub struct IssueCloseRequest {
    issue: ResolvedIssue,
    credential_handle: IssueCredentialHandleId,
    operation_id: IssueOperationId,
    fingerprint: IssueOperationFingerprint,
    source_merge: SourceMergeReceipt,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueCloseRequestWire {
    issue: ResolvedIssue,
    credential_handle: IssueCredentialHandleId,
    operation_id: IssueOperationId,
    fingerprint: IssueOperationFingerprint,
    source_merge: SourceMergeReceipt,
}

impl TryFrom<IssueCloseRequestWire> for IssueCloseRequest {
    type Error = IssueContractError;

    fn try_from(wire: IssueCloseRequestWire) -> Result<Self, Self::Error> {
        IssueContractError::checked(Self {
            issue: wire.issue,
            credential_handle: wire.credential_handle,
            operation_id: wire.operation_id,
            fingerprint: wire.fingerprint,
            source_merge: wire.source_merge,
        })
    }
}

impl IssueCloseRequest {
    pub fn new(
        issue: ResolvedIssue,
        credential_handle: IssueCredentialHandleId,
        identity: IssueOperationIdentity,
        source_merge: SourceMergeReceipt,
    ) -> Result<Self, IssueContractError> {
        let (operation_id, fingerprint) = identity;
        IssueContractError::checked(Self {
            issue,
            credential_handle,
            operation_id,
            fingerprint,
            source_merge,
        })
    }

    #[must_use]
    pub fn issue(&self) -> &ResolvedIssue {
        &self.issue
    }

    #[must_use]
    pub fn credential_handle(&self) -> &IssueCredentialHandleId {
        &self.credential_handle
    }

    #[must_use]
    pub fn operation_id(&self) -> &IssueOperationId {
        &self.operation_id
    }

    #[must_use]
    pub fn fingerprint(&self) -> &IssueOperationFingerprint {
        &self.fingerprint
    }

    #[must_use]
    pub fn source_merge(&self) -> &SourceMergeReceipt {
        &self.source_merge
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueClosedState {
    Closed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "IssueCloseReceiptWire")]
#[serde(rename_all = "camelCase")]
pub struct IssueCloseReceipt {
    issue: ResolvedIssue,
    operation_id: IssueOperationId,
    fingerprint: IssueOperationFingerprint,
    source_merge: SourceMergeReceipt,
    state: IssueClosedState,
    public_urls: BoundedVec<IssuePublicUrl>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueCloseReceiptWire {
    issue: ResolvedIssue,
    operation_id: IssueOperationId,
    fingerprint: IssueOperationFingerprint,
    source_merge: SourceMergeReceipt,
    state: IssueClosedState,
    public_urls: BoundedVec<IssuePublicUrl>,
}

impl TryFrom<IssueCloseReceiptWire> for IssueCloseReceipt {
    type Error = IssueContractError;

    fn try_from(wire: IssueCloseReceiptWire) -> Result<Self, Self::Error> {
        IssueContractError::checked(Self {
            issue: wire.issue,
            operation_id: wire.operation_id,
            fingerprint: wire.fingerprint,
            source_merge: wire.source_merge,
            state: wire.state,
            public_urls: wire.public_urls,
        })
    }
}

impl IssueCloseReceipt {
    pub fn new(
        issue: ResolvedIssue,
        identity: IssueOperationIdentity,
        source_merge: SourceMergeReceipt,
        public_urls: Vec<IssuePublicUrl>,
    ) -> Result<Self, IssueContractError> {
        let (operation_id, fingerprint) = identity;
        IssueContractError::checked(Self {
            issue,
            operation_id,
            fingerprint,
            source_merge,
            state: IssueClosedState::Closed,
            public_urls: BoundedVec::new(public_urls)
                .map_err(|error| IssueContractError::new("public URLs", error))?,
        })
    }

    #[must_use]
    pub fn issue(&self) -> &ResolvedIssue {
        &self.issue
    }

    #[must_use]
    pub fn operation_id(&self) -> &IssueOperationId {
        &self.operation_id
    }

    #[must_use]
    pub fn fingerprint(&self) -> &IssueOperationFingerprint {
        &self.fingerprint
    }

    #[must_use]
    pub fn source_merge(&self) -> &SourceMergeReceipt {
        &self.source_merge
    }

    #[must_use]
    pub fn state(&self) -> IssueClosedState {
        self.state
    }

    #[must_use]
    pub fn public_urls(&self) -> &[IssuePublicUrl] {
        self.public_urls.as_slice()
    }

    pub(super) fn matches_request(&self, request: &IssueCloseRequest) -> bool {
        self.issue() == request.issue()
            && self.operation_id() == request.operation_id()
            && self.fingerprint() == request.fingerprint()
            && self.source_merge() == request.source_merge()
    }
}
