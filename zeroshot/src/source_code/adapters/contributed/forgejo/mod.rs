//! Self-hosted Forgejo source provider.
use std::sync::Arc;

use crate::native_v2_contract::ResolvedSource;
use crate::native_v2_delivery::transport::GitTransport;
use crate::source_code_provider::adapters::core::{instance, network::ForgeNetwork};
use crate::source_code_provider::{
    SourceCodeProviderRegistry, SourceDeliveryAuthority, SourceProfileId, SourceProviderId,
    SourceProviderRef, SourceRegistryError,
};

mod access;
mod api;
#[cfg(test)]
mod api_tests;
mod authority;
#[cfg(test)]
mod authority_tests;
mod checks;
#[cfg(test)]
mod checks_tests;
mod ci;
#[cfg(test)]
mod ci_tests;
#[cfg(test)]
mod http_fixture;
mod merge;
#[cfg(test)]
mod merge_tests;
mod provider;
mod review;
#[cfg(test)]
mod review_tests;
#[cfg(test)]
mod tests;
mod wire;

use self::authority::ForgejoDeliveryAuthority;

pub(crate) const FORGEJO_PROVIDER_ID: &str = "source.forgejo";
pub(crate) const FORGEJO_CONNECTION: &str = "forgejo";
pub(crate) const FORGEJO_TOKEN_FIELD: &str = "FORGEJO_TOKEN";

pub(crate) fn register(
    registry: &mut SourceCodeProviderRegistry,
    source: Option<&ResolvedSource>,
    transport: Option<&GitTransport>,
) -> Result<(), SourceRegistryError> {
    let stored = source.filter(|source| {
        source
            .remote
            .as_ref()
            .and_then(|remote| remote.forge.as_ref())
            .is_some_and(|forge| forge.provider.as_str() == FORGEJO_PROVIDER_ID)
    });
    let authority = match (stored, transport) {
        (Some(source), Some(transport)) => Some(delivery_authority(source, transport)?),
        _ => None,
    };
    // Every descriptor value is static and valid, so no contract-error conversion is needed.
    registry.register(Arc::new(
        provider::ForgejoSourceProvider::new(source)
            .expect("static provider descriptor")
            .with_authority(authority),
    ))
}

/// An admitted instance and an admitted transport are both required; a metadata-only registration
/// keeps performing no effects.
fn delivery_authority(
    source: &ResolvedSource,
    transport: &GitTransport,
) -> Result<Arc<dyn SourceDeliveryAuthority>, SourceRegistryError> {
    let forge = source
        .remote
        .as_ref()
        .and_then(|remote| remote.forge.as_ref())
        .ok_or_else(unavailable)?;
    let base = instance::forge_base_url(forge).map_err(|_| unavailable())?;
    let authority =
        ForgejoDeliveryAuthority::new(base, Some(transport.clone())).map_err(|_| unavailable())?;
    let network = transport
        .config()
        .local_environment
        .as_ref()
        .map(ForgeNetwork::from_snapshot);
    Ok(Arc::new(
        authority
            .with_network(network.unwrap_or_default())
            .map_err(|_| unavailable())?,
    ))
}

fn unavailable() -> SourceRegistryError {
    SourceRegistryError::UnavailableDelivery {
        provider: SourceProviderRef::new(
            SourceProviderId::new(FORGEJO_PROVIDER_ID).expect("static provider id"),
            1,
        )
        .expect("static provider reference"),
        profile: SourceProfileId::new("default").expect("static profile id"),
    }
}
