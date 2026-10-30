//! Contributions use the same factories and runtime contracts as maintained harnesses.
pub mod hermes;
pub mod opencode;

#[cfg(all(test, unix))]
mod test_support;

pub(crate) fn register(registry: &mut crate::harnesses::HarnessRegistry) {
    registry
        .register(opencode::registration())
        .expect("compiled contributed harness registrations are unique");
    registry
        .register(hermes::registration())
        .expect("compiled contributed harness registrations are unique");
}
