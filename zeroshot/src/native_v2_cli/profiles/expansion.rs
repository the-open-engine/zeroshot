use openengine_cluster_protocol::{GraphSpec, RunProfileName, RuntimePlan, MAX_REQUEST_RESPONSE_BYTES};
use crate::native_v2_admission::profile_expansion::{expand_profile_imports, ProfileExpansionError};
use super::{NativeV2CliError, StoredProfiles, decode_profile};

pub(super) fn expand(
    stored: &StoredProfiles,
    graph: GraphSpec,
    runtime: RuntimePlan,
    root_name: Option<&RunProfileName>,
) -> Result<(GraphSpec, RuntimePlan), NativeV2CliError> {
    let mut imported_bytes = 0usize;
    let mut catalog_error = None;
    let expanded = expand_profile_imports(graph, runtime, root_name, |name| {
        let Some(raw) = stored.profiles.get(name) else {
            return Ok(None);
        };
        imported_bytes = imported_bytes.saturating_add(raw.get().len());
        if imported_bytes > MAX_REQUEST_RESPONSE_BYTES {
            return Err(ProfileExpansionError::new(
                "expanded profile imports exceed the request size limit",
            ));
        }
        match decode_profile(raw, name, false) {
            Ok(profile) => Ok(Some((profile.graph, profile.runtime))),
            Err(error) => {
                catalog_error = Some(error);
                Err(ProfileExpansionError::new("profile catalog lookup failed"))
            }
        }
    });
    if let Some(error) = catalog_error {
        return Err(error);
    }
    expanded.map_err(|error| NativeV2CliError::Usage(error.to_string()))
}
