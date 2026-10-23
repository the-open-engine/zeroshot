//! Host-owned, versioned lessons and deterministic resident retrieval.
//!
//! Provider adapters and postmortem orchestration depend on this module; the store
//! and matcher never invoke a model, a process, or a network service.

pub(crate) mod hooks;
pub(crate) mod postmortem;
pub(crate) mod runtime;

mod model;
mod retrieval;
mod store;

pub use model::*;
pub use retrieval::*;
pub use store::*;

#[cfg(test)]
mod tests;
