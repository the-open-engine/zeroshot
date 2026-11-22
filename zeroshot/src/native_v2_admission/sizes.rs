use openengine_cluster_protocol::{
    MAX_TARGET_HTTP_PROBLEM_MESSAGE_BYTES, RuntimePlan, TargetRunSizesDiscovery,
};

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
        let mut message = format!("this target does not offer run size {size}; supported sizes: ");
        // Include the error's display prefix in the HTTP problem's byte budget.
        let overhead = NativeV2AdmissionError::InvalidRuntime(String::new())
            .to_string()
            .len();
        for (index, offered) in catalog.sizes.iter().enumerate() {
            let separator = if index == 0 { "" } else { ", " };
            let remaining = catalog.sizes.len() - index - 1;
            let suffix = if remaining == 0 {
                String::new()
            } else {
                format!(", ... ({remaining} more)")
            };
            if overhead + message.len() + separator.len() + offered.id.as_str().len() + suffix.len()
                > MAX_TARGET_HTTP_PROBLEM_MESSAGE_BYTES
            {
                message.push_str(separator);
                message.push_str(&format!("... ({} more)", remaining + 1));
                break;
            }
            message.push_str(separator);
            message.push_str(offered.id.as_str());
        }
        if catalog.sizes.is_empty() {
            message.push_str("none");
        }
        return Err(NativeV2AdmissionError::InvalidRuntime(message));
    }
    runtime.size = Some(size.clone());
    Ok(())
}

#[cfg(test)]
#[path = "sizes_tests.rs"]
mod tests;
