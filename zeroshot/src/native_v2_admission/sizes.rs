use openengine_cluster_protocol::{RuntimePlan, TargetRunSizesDiscovery};

use super::NativeV2AdmissionError;

/// Resolves an omitted size from one target's catalog and rejects IDs that target does not offer.
/// Call this at admission, without changing the caller's original idempotency input.
pub fn resolve_runtime_size(
    runtime: &mut RuntimePlan,
    catalog: &TargetRunSizesDiscovery,
) -> Result<(), NativeV2AdmissionError> {
    catalog
        .validate()
        .map_err(|error| NativeV2AdmissionError::InvalidRuntime(error.to_string()))?;
    let size = runtime
        .size
        .as_ref()
        .or(catalog.default.as_ref())
        .ok_or_else(|| {
            NativeV2AdmissionError::InvalidRuntime("this target offers no run sizes".to_owned())
        })?;
    if !catalog.sizes.iter().any(|offered| &offered.id == size) {
        return Err(NativeV2AdmissionError::InvalidRuntime(format!(
            "this target does not offer run size {size}"
        )));
    }
    runtime.size = Some(size.clone());
    Ok(())
}

#[cfg(test)]
#[path = "sizes_tests.rs"]
mod tests;
