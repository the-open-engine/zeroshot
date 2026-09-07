use super::*;
use std::collections::BTreeMap;

fn validate_close_inspection(
    request: &IssueCloseRequest,
    inspection: IssueCloseInspection,
) -> Result<IssueCloseInspection, IssueCallError> {
    if let IssueCloseInspection::Applied(receipt) = &inspection {
        if !receipt.matches_request(request) {
            return Err(IssueCallError::InvalidEvidence {
                reason: "applied inspection changed issue, operation, fingerprint, or merge identity",
            });
        }
    }
    Ok(inspection)
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum IssueRegistryError {
    #[error("issue provider {provider} is already registered")]
    DuplicateRegistration { provider: IssueProviderRef },
    #[error("unknown issue provider id {id}")]
    UnknownProvider { id: IssueProviderId },
    #[error("issue provider version {provider} is unavailable")]
    UnavailableVersion { provider: IssueProviderRef },
    #[error("issue provider profile {profile} is unavailable for {provider}")]
    UnavailableProfile {
        provider: IssueProviderRef,
        profile: IssueProfileId,
    },
    #[error("issue provider {provider} profile {profile} does not support {capability:?}")]
    UnsupportedCapability {
        provider: IssueProviderRef,
        profile: IssueProfileId,
        capability: IssueCapability,
    },
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum IssueCallError {
    #[error(transparent)]
    Registry(#[from] IssueRegistryError),
    #[error(transparent)]
    Provider(#[from] IssueProviderFailure),
    #[error("issue close cannot be invoked from inspection state {inspection:?}")]
    UnsafeToInvoke { inspection: IssueCloseInspection },
    #[error("issue provider returned evidence that does not match the request: {reason}")]
    InvalidEvidence { reason: &'static str },
}

#[derive(Default)]
pub struct IssueProviderRegistry {
    providers: BTreeMap<IssueProviderId, BTreeMap<u32, Arc<dyn IssueProvider>>>,
}

impl IssueProviderRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, provider: Arc<dyn IssueProvider>) -> Result<(), IssueRegistryError> {
        let reference = provider.descriptor().provider().clone();
        let versions = self.providers.entry(reference.id().clone()).or_default();
        if versions.contains_key(&reference.version()) {
            return Err(IssueRegistryError::DuplicateRegistration {
                provider: reference,
            });
        }
        versions.insert(reference.version(), provider);
        Ok(())
    }

    pub fn lookup(
        &self,
        reference: &IssueProviderRef,
    ) -> Result<Arc<dyn IssueProvider>, IssueRegistryError> {
        let versions = self.providers.get(reference.id()).ok_or_else(|| {
            IssueRegistryError::UnknownProvider {
                id: reference.id().clone(),
            }
        })?;
        versions.get(&reference.version()).cloned().ok_or_else(|| {
            IssueRegistryError::UnavailableVersion {
                provider: reference.clone(),
            }
        })
    }

    pub fn descriptor(
        &self,
        reference: &IssueProviderRef,
    ) -> Result<&IssueProviderDescriptor, IssueRegistryError> {
        let versions = self.providers.get(reference.id()).ok_or_else(|| {
            IssueRegistryError::UnknownProvider {
                id: reference.id().clone(),
            }
        })?;
        versions
            .get(&reference.version())
            .map(|provider| provider.descriptor())
            .ok_or_else(|| IssueRegistryError::UnavailableVersion {
                provider: reference.clone(),
            })
    }

    pub fn capability(
        &self,
        reference: &IssueProviderRef,
        profile: &IssueProfileId,
        capability: IssueCapability,
    ) -> Result<&IssueProfileDescriptor, IssueRegistryError> {
        let descriptor = self.descriptor(reference)?;
        let profile_descriptor =
            descriptor
                .profile(profile)
                .ok_or_else(|| IssueRegistryError::UnavailableProfile {
                    provider: reference.clone(),
                    profile: profile.clone(),
                })?;
        if !profile_descriptor.supports(capability) {
            return Err(IssueRegistryError::UnsupportedCapability {
                provider: reference.clone(),
                profile: profile.clone(),
                capability,
            });
        }
        Ok(profile_descriptor)
    }

    fn provider_for(
        &self,
        reference: &IssueProviderRef,
        profile: &IssueProfileId,
        capability: IssueCapability,
    ) -> Result<Arc<dyn IssueProvider>, IssueRegistryError> {
        self.capability(reference, profile, capability)?;
        self.lookup(reference)
    }

    pub async fn resolve(
        &self,
        request: &IssueResolveRequest,
    ) -> Result<ResolvedIssue, IssueCallError> {
        let provider =
            self.provider_for(request.provider(), request.profile(), IssueCapability::Read)?;
        let issue = provider.resolve(request).await?;
        if issue.provider() != request.provider()
            || issue.profile() != request.profile()
            || issue.account() != request.account()
        {
            return Err(IssueCallError::InvalidEvidence {
                reason: "resolved issue changed provider, profile, or account identity",
            });
        }
        Ok(issue)
    }

    pub async fn inspect_close(
        &self,
        request: &IssueCloseRequest,
    ) -> Result<IssueCloseInspection, IssueCallError> {
        let issue = request.issue();
        let provider =
            self.provider_for(issue.provider(), issue.profile(), IssueCapability::Close)?;
        validate_close_inspection(request, provider.inspect_close(request).await?)
    }

    pub async fn close(
        &self,
        request: &IssueCloseRequest,
    ) -> Result<IssueCloseReceipt, IssueCallError> {
        let issue = request.issue();
        let profile_descriptor =
            self.capability(issue.provider(), issue.profile(), IssueCapability::Close)?;
        let native_idempotency =
            profile_descriptor.has_provider_native_idempotency(IssueCapability::Close);
        let provider = self.lookup(issue.provider())?;
        let inspection =
            validate_close_inspection(request, provider.inspect_close(request).await?)?;
        let inspection = match inspection {
            IssueCloseInspection::Applied(receipt) => return Ok(*receipt),
            other => other,
        };
        if !inspection.permits_invocation(native_idempotency) {
            return Err(IssueCallError::UnsafeToInvoke { inspection });
        }
        let receipt = provider.close(request).await?;
        if !receipt.matches_request(request) {
            return Err(IssueCallError::InvalidEvidence {
                reason: "close receipt changed issue, operation, fingerprint, or merge identity",
            });
        }
        Ok(receipt)
    }
}
