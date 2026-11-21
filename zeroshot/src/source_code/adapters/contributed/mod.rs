//! Compile-time contributor registration. Add adapters here; the common workflow and node types
//! remain unchanged. A metadata-only context must perform no filesystem, process, or network work.
use crate::source_code_provider::{SourceCodeProviderRegistry, SourceRegistryError};
use super::SourceAdapterContext;

pub(crate) mod forgejo;

pub fn register(
    registry: &mut SourceCodeProviderRegistry,
    context: SourceAdapterContext<'_>,
) -> Result<(), SourceRegistryError> {
    forgejo::register(registry, context.source, context.transport)?;
    #[cfg(test)]
    registration_tests::register(registry, context)?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod registration_tests;
