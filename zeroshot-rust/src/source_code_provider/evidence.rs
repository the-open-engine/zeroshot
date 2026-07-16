use super::*;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(try_from = "SourceOperationReceiptWire")]
#[serde(rename_all = "snake_case", tag = "kind", content = "receipt")]
pub enum SourceOperationReceipt {
    Applied(SourceAppliedReceipt),
    Merge(SourceMergeReceipt),
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "receipt")]
enum SourceOperationReceiptRef<'a> {
    Applied(&'a SourceAppliedReceipt),
    Merge(&'a SourceMergeReceipt),
}

impl Serialize for SourceOperationReceipt {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = match self {
            Self::Applied(receipt) => SourceOperationReceiptRef::Applied(receipt),
            Self::Merge(receipt) => SourceOperationReceiptRef::Merge(receipt),
        };
        validate_serialized(&wire).map_err(ser::Error::custom)?;
        wire.serialize(serializer)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "receipt")]
enum SourceOperationReceiptWire {
    Applied(SourceAppliedReceipt),
    Merge(SourceMergeReceipt),
}

impl TryFrom<SourceOperationReceiptWire> for SourceOperationReceipt {
    type Error = SourceContractError;

    fn try_from(wire: SourceOperationReceiptWire) -> Result<Self, Self::Error> {
        SourceContractError::checked(match wire {
            SourceOperationReceiptWire::Applied(receipt) => Self::Applied(receipt),
            SourceOperationReceiptWire::Merge(receipt) => Self::Merge(receipt),
        })
    }
}

impl SourceOperationReceipt {
    #[must_use]
    pub fn capability(&self) -> SourceCapability {
        match self {
            Self::Applied(receipt) => receipt.capability(),
            Self::Merge(_) => SourceCapability::Merge,
        }
    }

    fn operation_identity(
        &self,
    ) -> (
        &CanonicalRepository,
        &SourceOperationId,
        &SourceOperationFingerprint,
    ) {
        match self {
            Self::Applied(receipt) => (
                receipt.repository(),
                receipt.operation_id(),
                receipt.fingerprint(),
            ),
            Self::Merge(receipt) => (
                receipt.repository(),
                receipt.operation_id(),
                receipt.fingerprint(),
            ),
        }
    }

    fn matches_operation(&self, operation: &SourceOperation) -> bool {
        match (self, operation) {
            (
                Self::Merge(receipt),
                SourceOperation::Merge {
                    expected_base,
                    expected_head,
                },
            ) => {
                receipt.expected_base() == expected_base && receipt.expected_head() == expected_head
            }
            (Self::Applied(receipt), _) => receipt.capability() == operation.capability(),
            (Self::Merge(_), _) => false,
        }
    }

    pub(super) fn matches_request(&self, request: &SourceOperationRequest) -> bool {
        let (repository, operation_id, fingerprint) = self.operation_identity();
        repository == request.repository()
            && operation_id == request.operation_id()
            && fingerprint == request.fingerprint()
            && self.matches_operation(request.operation())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(try_from = "SourceOperationInspectionWire")]
#[serde(rename_all = "snake_case", tag = "state", content = "evidence")]
pub enum SourceOperationInspection {
    Unobserved,
    Pending,
    Applied(Box<SourceOperationReceipt>),
    Conflict {
        observed_fingerprint: SourceOperationFingerprint,
    },
    Indeterminate {
        evidence: SourceFailureMessage,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "evidence")]
enum SourceOperationInspectionRef<'a> {
    Unobserved,
    Pending,
    Applied(&'a SourceOperationReceipt),
    Conflict {
        observed_fingerprint: &'a SourceOperationFingerprint,
    },
    Indeterminate {
        evidence: &'a SourceFailureMessage,
    },
}

impl Serialize for SourceOperationInspection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = match self {
            Self::Unobserved => SourceOperationInspectionRef::Unobserved,
            Self::Pending => SourceOperationInspectionRef::Pending,
            Self::Applied(receipt) => SourceOperationInspectionRef::Applied(receipt),
            Self::Conflict {
                observed_fingerprint,
            } => SourceOperationInspectionRef::Conflict {
                observed_fingerprint,
            },
            Self::Indeterminate { evidence } => {
                SourceOperationInspectionRef::Indeterminate { evidence }
            }
        };
        validate_serialized(&wire).map_err(ser::Error::custom)?;
        wire.serialize(serializer)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "evidence")]
enum SourceOperationInspectionWire {
    Unobserved,
    Pending,
    Applied(Box<SourceOperationReceipt>),
    Conflict {
        observed_fingerprint: SourceOperationFingerprint,
    },
    Indeterminate {
        evidence: SourceFailureMessage,
    },
}

impl TryFrom<SourceOperationInspectionWire> for SourceOperationInspection {
    type Error = SourceContractError;

    fn try_from(wire: SourceOperationInspectionWire) -> Result<Self, Self::Error> {
        SourceContractError::checked(match wire {
            SourceOperationInspectionWire::Unobserved => Self::Unobserved,
            SourceOperationInspectionWire::Pending => Self::Pending,
            SourceOperationInspectionWire::Applied(receipt) => Self::Applied(receipt),
            SourceOperationInspectionWire::Conflict {
                observed_fingerprint,
            } => Self::Conflict {
                observed_fingerprint,
            },
            SourceOperationInspectionWire::Indeterminate { evidence } => {
                Self::Indeterminate { evidence }
            }
        })
    }
}

impl SourceOperationInspection {
    #[must_use]
    pub fn permits_invocation(&self, provider_native_idempotency: bool) -> bool {
        matches!(self, Self::Unobserved)
            || (provider_native_idempotency
                && matches!(self, Self::Pending | Self::Indeterminate { .. }))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProviderFailureCode {
    Unavailable,
    Unauthorized,
    InvalidRequest,
    Conflict,
    Indeterminate,
}

#[derive(Clone, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
#[error("source provider {code:?}: {message}")]
#[serde(rename_all = "camelCase")]
pub struct SourceProviderFailure {
    code: SourceProviderFailureCode,
    message: SourceFailureMessage,
}

impl SourceProviderFailure {
    pub fn new(
        code: SourceProviderFailureCode,
        message: SourceFailureMessage,
    ) -> Result<Self, SourceContractError> {
        SourceContractError::checked(Self { code, message })
    }

    #[must_use]
    pub fn code(&self) -> SourceProviderFailureCode {
        self.code
    }

    #[must_use]
    pub fn message(&self) -> &SourceFailureMessage {
        &self.message
    }
}

#[async_trait]
pub trait SourceCodeProvider: Send + Sync {
    fn descriptor(&self) -> &SourceProviderDescriptor;

    async fn identify_repository(
        &self,
        request: &SourceIdentifyRepositoryRequest,
    ) -> Result<CanonicalRepository, SourceProviderFailure>;

    async fn inspect_repository(
        &self,
        request: &SourceInspectRepositoryRequest,
    ) -> Result<SourceRepositoryInspection, SourceProviderFailure>;

    async fn materialize(
        &self,
        request: &SourceMaterializeRequest,
        destination: SourceMaterializationDestination<'_>,
    ) -> Result<SourceMaterializationReceipt, SourceProviderFailure>;

    async fn inspect_operation(
        &self,
        request: &SourceOperationRequest,
    ) -> Result<SourceOperationInspection, SourceProviderFailure>;

    async fn operate(
        &self,
        request: &SourceOperationRequest,
    ) -> Result<SourceOperationReceipt, SourceProviderFailure>;
}
