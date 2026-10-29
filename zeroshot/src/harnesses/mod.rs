//! Host-selected harness registrations. Core and contributed adapters share execution contracts.

pub mod adapters;
mod contracts;
mod registry;
pub(crate) mod runtime;

pub use contracts::{HarnessAdapter, HarnessBuildContext, HarnessConfig, HarnessError, HarnessHost};
pub use registry::{HarnessDescriptor, HarnessRegistration, HarnessRegistry, HarnessSource};
pub(crate) use contracts::ProviderAccessContract;

#[cfg(test)]
mod tests;

#[cfg(all(test, unix))]
mod live_tests;
