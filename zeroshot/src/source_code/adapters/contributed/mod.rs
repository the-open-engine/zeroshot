//! Compile-time contributor registration. Add adapters here; the common workflow and node types
//! remain unchanged. A metadata-only context must perform no filesystem, process, or network work.
use crate::source_code_provider::{SourceCodeProviderRegistry, SourceRegistryError};
use super::SourceAdapterContext;

pub fn register(
    _registry: &mut SourceCodeProviderRegistry,
    _context: SourceAdapterContext<'_>,
) -> Result<(), SourceRegistryError> {
    #[cfg(test)]
    registration_tests::register(_registry, _context)?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod registration_tests;
