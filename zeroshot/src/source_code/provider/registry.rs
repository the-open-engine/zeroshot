use super::*;
use std::collections::BTreeMap;
use crate::provider_value::validate_serialized;

fn validate_operation_inspection(
    request: &SourceOperationRequest,
    inspection: SourceOperationInspection,
) -> Result<SourceOperationInspection, SourceCallError> {
    if validate_serialized(&inspection).is_err() {
        return Err(SourceCallError::InvalidEvidence {
            reason: "operation inspection exceeded the canonical serialized bound",
        });
    }
    if let SourceOperationInspection::Applied(receipt) = &inspection {
        if !receipt.matches_request(request) {
            return Err(SourceCallError::InvalidEvidence {
                reason: "applied inspection changed exact source operation authority",
            });
        }
    }
    Ok(inspection)
}

async fn inspect_after_effect(
    provider: &dyn SourceCodeProvider,
    request: &SourceOperationRequest,
    outcome: SourceInvocationOutcome,
    reason: &'static str,
) -> Result<SourceOperationInspection, SourceCallError> {
    let observed = provider
        .inspect_operation(request)
        .await
        .map_err(|failure| SourceCallError::PostEffectInspectionFailed { outcome, failure })?;
    validate_operation_inspection(request, observed)
        .map_err(|_| SourceCallError::PostEffectInvalidEvidence { outcome, reason })
}

async fn reconcile_returned_receipt(
    provider: &dyn SourceCodeProvider,
    request: &SourceOperationRequest,
    receipt: SourceOperationReceipt,
) -> Result<SourceOperationReceipt, SourceCallError> {
    let outcome = SourceInvocationOutcome::ReturnedReceipt;
    let authoritative = inspect_after_effect(
        provider,
        request,
        outcome,
        "post-effect inspection changed exact source operation authority",
    )
    .await?;
    match authoritative {
        SourceOperationInspection::Applied(observed)
            if receipt.matches_request(request) && *observed == receipt =>
        {
            Ok(receipt)
        }
        SourceOperationInspection::Applied(_) => Err(SourceCallError::PostEffectInvalidEvidence {
            outcome,
            reason: "direct and inspected receipts did not prove the same exact authority",
        }),
        inspection => Err(SourceCallError::UnreconciledUncertainty {
            outcome,
            inspection,
        }),
    }
}

async fn perform_source_operation(
    provider: &dyn SourceCodeProvider,
    request: &SourceOperationRequest,
    workspace: SourceWorkspaceCapability<'_>,
) -> Result<SourceOperationReceipt, SourceCallError> {
    let performed = provider.operate(request, workspace).await;
    match performed {
        Ok(receipt) => reconcile_returned_receipt(provider, request, receipt).await,
        Err(failure) if failure.code() == SourceProviderFailureCode::Indeterminate => {
            reconcile_indeterminate_operation(provider, request).await
        }
        Err(failure) => Err(SourceCallError::Provider(failure)),
    }
}

async fn reconcile_indeterminate_operation(
    provider: &dyn SourceCodeProvider,
    request: &SourceOperationRequest,
) -> Result<SourceOperationReceipt, SourceCallError> {
    let outcome = SourceInvocationOutcome::Indeterminate;
    let authoritative = inspect_after_effect(
        provider,
        request,
        outcome,
        "post-uncertainty inspection changed exact source operation authority",
    )
    .await?;
    match authoritative {
        SourceOperationInspection::Applied(receipt) => Ok(*receipt),
        inspection => Err(SourceCallError::UnreconciledUncertainty {
            outcome,
            inspection,
        }),
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SourceRegistryError {
    #[error("no registered source provider recognizes this comment URL")]
    UnsupportedCommentUrl,
    #[error("multiple source providers recognize this comment URL")]
    AmbiguousCommentUrl,
    #[error("multiple source providers recognize the selected Git remote")]
    AmbiguousRemote,
    #[error("source provider {provider:?} profile {profile:?} has no configured delivery adapter")]
    UnavailableDelivery {
        provider: SourceProviderRef,
        profile: SourceProfileId,
    },

    #[error("source provider {provider} is already registered")]
    DuplicateRegistration { provider: SourceProviderRef },
    #[error("unknown source provider id {id}")]
    UnknownProvider { id: SourceProviderId },
    #[error("source provider version {provider} is unavailable")]
    UnavailableVersion { provider: SourceProviderRef },
    #[error("source provider profile {profile} is unavailable for {provider}")]
    UnavailableProfile {
        provider: SourceProviderRef,
        profile: SourceProfileId,
    },
    #[error("source provider {provider} profile {profile} does not support {capability:?}")]
    UnsupportedCapability {
        provider: SourceProviderRef,
        profile: SourceProfileId,
        capability: SourceCapability,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceInvocationOutcome {
    ReturnedReceipt,
    Indeterminate,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum SourceCallError {
    #[error(transparent)]
    Registry(#[from] SourceRegistryError),
    #[error(transparent)]
    Provider(#[from] SourceProviderFailure),
    #[error("source operation cannot be invoked from inspection state {inspection:?}")]
    UnsafeToInvoke {
        inspection: SourceOperationInspection,
    },
    #[error("source workspace capability does not match operation workspace")]
    WorkspaceMismatch,
    #[error(
        "source operation remains uncertain after {outcome:?} and authoritative inspection: {inspection:?}"
    )]
    UnreconciledUncertainty {
        outcome: SourceInvocationOutcome,
        inspection: SourceOperationInspection,
    },
    #[error("source operation authoritative inspection failed after {outcome:?}: {failure}")]
    PostEffectInspectionFailed {
        outcome: SourceInvocationOutcome,
        failure: SourceProviderFailure,
    },
    #[error("source operation evidence is invalid after {outcome:?}: {reason}")]
    PostEffectInvalidEvidence {
        outcome: SourceInvocationOutcome,
        reason: &'static str,
    },
    #[error("source provider returned evidence that does not match the request: {reason}")]
    InvalidEvidence { reason: &'static str },
}

#[derive(Default)]
pub struct SourceCodeProviderRegistry {
    providers: BTreeMap<SourceProviderId, BTreeMap<u32, Arc<dyn SourceCodeProvider>>>,
}

impl SourceCodeProviderRegistry {
    pub fn comment_destination(
        &self,
        url: &str,
    ) -> Result<SourceCommentDestination, SourceRegistryError> {
        let mut selected = None;
        for provider in self.providers.values().flat_map(BTreeMap::values) {
            if let Some(destination) = provider.comment_destination_from_url(url) {
                if selected.is_some() {
                    return Err(SourceRegistryError::AmbiguousCommentUrl);
                }
                selected = Some(destination);
            }
        }
        let destination = selected.ok_or(SourceRegistryError::UnsupportedCommentUrl)?;
        self.capability(
            &destination.provider,
            &SourceProfileId::new("default").expect("static profile"),
            SourceCapability::Comment,
        )?;
        Ok(destination)
    }

    pub fn comment_authority(
        &self,
        destination: &SourceCommentDestination,
    ) -> Result<Arc<dyn SourceCommentAuthority>, SourceRegistryError> {
        let profile = SourceProfileId::new("default").expect("static profile");
        self.capability(&destination.provider, &profile, SourceCapability::Comment)?;
        self.lookup(&destination.provider)?
            .comment_authority()
            .ok_or_else(|| SourceRegistryError::UnavailableDelivery {
                provider: destination.provider.clone(),
                profile,
            })
    }

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        provider: Arc<dyn SourceCodeProvider>,
    ) -> Result<(), SourceRegistryError> {
        let reference = provider.descriptor().provider().clone();
        let versions = self.providers.entry(reference.id().clone()).or_default();
        if versions.contains_key(&reference.version()) {
            return Err(SourceRegistryError::DuplicateRegistration {
                provider: reference,
            });
        }
        versions.insert(reference.version(), provider);
        Ok(())
    }

    /// Uses the registered providers' pure locator identification before credential resolution.
    pub fn repository_from_remote(
        &self,
        remote: &str,
    ) -> Result<Option<(SourceProviderRef, SourceRepositoryId)>, SourceRegistryError> {
        let mut matched = None;
        for provider in self.providers.values().flat_map(BTreeMap::values) {
            let Some(repository) = provider.repository_from_remote(remote) else {
                continue;
            };
            if matched.is_some() {
                return Err(SourceRegistryError::AmbiguousRemote);
            }
            matched = Some((provider.descriptor().provider().clone(), repository));
        }
        Ok(matched)
    }

    pub fn lookup(
        &self,
        reference: &SourceProviderRef,
    ) -> Result<Arc<dyn SourceCodeProvider>, SourceRegistryError> {
        let versions = self.providers.get(reference.id()).ok_or_else(|| {
            SourceRegistryError::UnknownProvider {
                id: reference.id().clone(),
            }
        })?;
        versions.get(&reference.version()).cloned().ok_or_else(|| {
            SourceRegistryError::UnavailableVersion {
                provider: reference.clone(),
            }
        })
    }

    pub fn descriptor(
        &self,
        reference: &SourceProviderRef,
    ) -> Result<&SourceProviderDescriptor, SourceRegistryError> {
        let versions = self.providers.get(reference.id()).ok_or_else(|| {
            SourceRegistryError::UnknownProvider {
                id: reference.id().clone(),
            }
        })?;
        versions
            .get(&reference.version())
            .map(|provider| provider.descriptor())
            .ok_or_else(|| SourceRegistryError::UnavailableVersion {
                provider: reference.clone(),
            })
    }

    pub fn capability(
        &self,
        reference: &SourceProviderRef,
        profile: &SourceProfileId,
        capability: SourceCapability,
    ) -> Result<&SourceProfileDescriptor, SourceRegistryError> {
        let descriptor = self.descriptor(reference)?;
        let profile_descriptor =
            descriptor
                .profile(profile)
                .ok_or_else(|| SourceRegistryError::UnavailableProfile {
                    provider: reference.clone(),
                    profile: profile.clone(),
                })?;
        if !profile_descriptor.supports(capability) {
            return Err(SourceRegistryError::UnsupportedCapability {
                provider: reference.clone(),
                profile: profile.clone(),
                capability,
            });
        }
        Ok(profile_descriptor)
    }

    fn provider_for(
        &self,
        reference: &SourceProviderRef,
        profile: &SourceProfileId,
        capability: SourceCapability,
    ) -> Result<Arc<dyn SourceCodeProvider>, SourceRegistryError> {
        self.capability(reference, profile, capability)?;
        self.lookup(reference)
    }

    /// Resolves trusted delivery through this same registry after checking every required capability.
    pub fn delivery_authority(
        &self,
        reference: &SourceProviderRef,
        profile: &SourceProfileId,
        required: &[SourceCapability],
    ) -> Result<Arc<dyn SourceDeliveryAuthority>, SourceRegistryError> {
        for capability in required {
            self.capability(reference, profile, *capability)?;
        }
        self.lookup(reference)?.delivery_authority().ok_or_else(|| {
            SourceRegistryError::UnavailableDelivery {
                provider: reference.clone(),
                profile: profile.clone(),
            }
        })
    }

    pub async fn identify_repository(
        &self,
        request: &SourceIdentifyRepositoryRequest,
    ) -> Result<CanonicalRepository, SourceCallError> {
        let provider = self.provider_for(
            request.provider(),
            request.profile(),
            SourceCapability::Read,
        )?;
        let repository = provider.identify_repository(request).await?;
        if repository.provider() != request.provider()
            || repository.profile() != request.profile()
            || repository.account() != request.account()
        {
            return Err(SourceCallError::InvalidEvidence {
                reason: "canonical repository identity changed provider, profile, or account",
            });
        }
        Ok(repository)
    }

    pub async fn inspect_repository(
        &self,
        request: &SourceInspectRepositoryRequest,
    ) -> Result<SourceRepositoryInspection, SourceCallError> {
        let repository = request.repository();
        let provider = self.provider_for(
            repository.provider(),
            repository.profile(),
            SourceCapability::Read,
        )?;
        let inspection = provider.inspect_repository(request).await?;
        if inspection.repository() != repository {
            return Err(SourceCallError::InvalidEvidence {
                reason: "repository inspection changed canonical repository identity",
            });
        }
        Ok(inspection)
    }

    pub async fn materialize(
        &self,
        request: &SourceMaterializeRequest,
        destination: SourceMaterializationDestination<'_>,
    ) -> Result<SourceMaterializationReceipt, SourceCallError> {
        let repository = request.repository();
        let provider = self.provider_for(
            repository.provider(),
            repository.profile(),
            SourceCapability::Read,
        )?;
        let receipt = provider.materialize(request, destination).await?;
        if receipt.repository() != repository || receipt.revision() != request.revision() {
            return Err(SourceCallError::InvalidEvidence {
                reason: "materialization receipt changed repository or revision identity",
            });
        }
        Ok(receipt)
    }

    pub async fn inspect_operation(
        &self,
        request: &SourceOperationRequest,
    ) -> Result<SourceOperationInspection, SourceCallError> {
        let repository = request.repository();
        let provider = self.provider_for(
            repository.provider(),
            repository.profile(),
            request.operation().capability(),
        )?;
        validate_operation_inspection(request, provider.inspect_operation(request).await?)
    }

    pub async fn operate(
        &self,
        request: &SourceOperationRequest,
        workspace: SourceWorkspaceCapability<'_>,
    ) -> Result<SourceOperationReceipt, SourceCallError> {
        if workspace.workspace() != request.workspace() {
            return Err(SourceCallError::WorkspaceMismatch);
        }
        let repository = request.repository();
        let capability = request.operation().capability();
        let profile = self.capability(repository.provider(), repository.profile(), capability)?;
        let native_idempotency = profile.has_provider_native_idempotency(capability);
        let provider = self.lookup(repository.provider())?;
        let inspection =
            validate_operation_inspection(request, provider.inspect_operation(request).await?)?;
        let inspection = match inspection {
            SourceOperationInspection::Applied(receipt) => return Ok(*receipt),
            other => other,
        };
        if !inspection.permits_invocation(native_idempotency) {
            return Err(SourceCallError::UnsafeToInvoke { inspection });
        }

        perform_source_operation(provider.as_ref(), request, workspace).await
    }
}

impl From<ValueError> for SourceContractError {
    fn from(error: ValueError) -> Self {
        Self::new("source provider value", error)
    }
}

#[cfg(test)]
#[path = "registry/tests.rs"]
mod tests;
