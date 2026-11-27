//! Compile-time contributor registration. Add adapters here; the common workflow and node types
//! remain unchanged. A metadata-only context must perform no filesystem, process, or network work.
use crate::source_code_provider::{SourceCodeProviderRegistry, SourceProviderRef, SourceRegistryError};

use super::SourceAdapterContext;

mod forgejo;

pub fn register(
    registry: &mut SourceCodeProviderRegistry,
    context: SourceAdapterContext<'_>,
) -> Result<(), SourceRegistryError> {
    register_with_selection(registry, context, None)
}

pub(super) fn register_with_selection(
    registry: &mut SourceCodeProviderRegistry,
    context: SourceAdapterContext<'_>,
    selected: Option<&SourceProviderRef>,
) -> Result<(), SourceRegistryError> {
    forgejo::register(registry, context.source, context.transport, selected)?;
    #[cfg(test)]
    registration_tests::register(registry, context)?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod registration_tests;
