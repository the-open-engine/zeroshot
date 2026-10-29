//! Explicit compile-time registration; no runtime directory scanning or code loading.
#[cfg(feature = "contributed-harnesses")]
pub mod contributed;
pub mod core;
