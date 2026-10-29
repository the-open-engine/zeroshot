//! Contributions use the same factories and runtime contracts as maintained harnesses.
pub mod opencode;

pub(crate) fn register(registry: &mut crate::harnesses::HarnessRegistry) {
    registry
        .register(opencode::registration())
        .expect("compiled contributed harness registrations are unique");
}
