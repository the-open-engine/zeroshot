//! Maintained and contributor source-provider adapters. Registration uses SourceCodeProviderRegistry.
pub mod contributed;
pub mod core;

/// A contribution can advertise metadata in pure preflight and bind effects only once the engine
/// supplies this source-scoped transport. The context is runtime-only and carries no credential.
pub struct SourceAdapterContext<'a> {
    pub source: Option<&'a crate::native_v2_contract::ResolvedSource>,
    pub transport: Option<&'a crate::native_v2_delivery::transport::GitTransport>,
}
