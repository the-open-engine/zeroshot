use super::*;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(try_from = "IssueCloseInspectionWire")]
#[serde(rename_all = "snake_case", tag = "state", content = "evidence")]
pub enum IssueCloseInspection {
    Unobserved,
    Pending,
    Applied(Box<IssueCloseReceipt>),
    Conflict {
        observed_fingerprint: IssueOperationFingerprint,
    },
    Indeterminate {
        evidence: IssueFailureMessage,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "evidence")]
enum IssueCloseInspectionRef<'a> {
    Unobserved,
    Pending,
    Applied(&'a IssueCloseReceipt),
    Conflict {
        observed_fingerprint: &'a IssueOperationFingerprint,
    },
    Indeterminate {
        evidence: &'a IssueFailureMessage,
    },
}

impl Serialize for IssueCloseInspection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = match self {
            Self::Unobserved => IssueCloseInspectionRef::Unobserved,
            Self::Pending => IssueCloseInspectionRef::Pending,
            Self::Applied(receipt) => IssueCloseInspectionRef::Applied(receipt),
            Self::Conflict {
                observed_fingerprint,
            } => IssueCloseInspectionRef::Conflict {
                observed_fingerprint,
            },
            Self::Indeterminate { evidence } => IssueCloseInspectionRef::Indeterminate { evidence },
        };
        validate_serialized(&wire).map_err(ser::Error::custom)?;
        wire.serialize(serializer)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "evidence")]
enum IssueCloseInspectionWire {
    Unobserved,
    Pending,
    Applied(Box<IssueCloseReceipt>),
    Conflict {
        observed_fingerprint: IssueOperationFingerprint,
    },
    Indeterminate {
        evidence: IssueFailureMessage,
    },
}

impl TryFrom<IssueCloseInspectionWire> for IssueCloseInspection {
    type Error = IssueContractError;

    fn try_from(wire: IssueCloseInspectionWire) -> Result<Self, Self::Error> {
        IssueContractError::checked(match wire {
            IssueCloseInspectionWire::Unobserved => Self::Unobserved,
            IssueCloseInspectionWire::Pending => Self::Pending,
            IssueCloseInspectionWire::Applied(receipt) => Self::Applied(receipt),
            IssueCloseInspectionWire::Conflict {
                observed_fingerprint,
            } => Self::Conflict {
                observed_fingerprint,
            },
            IssueCloseInspectionWire::Indeterminate { evidence } => {
                Self::Indeterminate { evidence }
            }
        })
    }
}

impl IssueCloseInspection {
    #[must_use]
    pub fn permits_invocation(&self, provider_native_idempotency: bool) -> bool {
        matches!(self, Self::Unobserved)
            || (provider_native_idempotency
                && matches!(self, Self::Pending | Self::Indeterminate { .. }))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueProviderFailureCode {
    Unavailable,
    Unauthorized,
    InvalidRequest,
    Conflict,
    Indeterminate,
}

#[derive(Clone, Debug, Deserialize, Eq, Error, PartialEq, Serialize)]
#[error("issue provider {code:?}: {message}")]
#[serde(rename_all = "camelCase")]
pub struct IssueProviderFailure {
    code: IssueProviderFailureCode,
    message: IssueFailureMessage,
}

impl IssueProviderFailure {
    pub fn new(
        code: IssueProviderFailureCode,
        message: IssueFailureMessage,
    ) -> Result<Self, IssueContractError> {
        IssueContractError::checked(Self { code, message })
    }

    #[must_use]
    pub fn code(&self) -> IssueProviderFailureCode {
        self.code
    }

    #[must_use]
    pub fn message(&self) -> &IssueFailureMessage {
        &self.message
    }
}

#[async_trait]
pub trait IssueProvider: Send + Sync {
    fn descriptor(&self) -> &IssueProviderDescriptor;

    async fn resolve(
        &self,
        request: &IssueResolveRequest,
    ) -> Result<ResolvedIssue, IssueProviderFailure>;

    async fn inspect_close(
        &self,
        request: &IssueCloseRequest,
    ) -> Result<IssueCloseInspection, IssueProviderFailure>;

    async fn close(
        &self,
        request: &IssueCloseRequest,
    ) -> Result<IssueCloseReceipt, IssueProviderFailure>;
}
