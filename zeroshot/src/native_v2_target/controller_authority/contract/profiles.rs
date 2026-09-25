use openengine_cluster_protocol::{
    RUN_PROFILES_KIND, TargetDiscoveryExtensions, EnvironmentId, RunProfileScope,
    RUNTIME_ENVIRONMENTS_KIND,
};
use reqwest::Url;

use super::{
    authority_error, capability_base_url, compile_literal_route, valid_literal_route_segment,
    validate_route_template,
};
use crate::native_v2_target::TargetAuthorityError;

#[derive(Clone)]
pub(in super::super) struct RunProfilesDescriptor {
    pub(in super::super) list: Url,
    pub(in super::super) show: Url,
    pub(in super::super) set: Url,
    pub(in super::super) delete: Url,
    pub(in super::super) default: Url,
    pub(in super::super) run: Url,
}

pub(super) fn build_profiles_descriptor(
    origin: &Url,
    extensions: &TargetDiscoveryExtensions,
) -> Result<Option<RunProfilesDescriptor>, TargetAuthorityError> {
    let Some(wire) = extensions.run_profiles.as_ref() else {
        return Ok(None);
    };
    if wire.kind != RUN_PROFILES_KIND {
        return Err(authority_error("run-profile discovery is incompatible"));
    }
    let base_url = capability_base_url(origin, &wire.base_url)?;
    Ok(Some(RunProfilesDescriptor {
        list: compile_literal_route(&base_url, &wire.route_templates.list, "run-profile")?,
        show: compile_literal_route(&base_url, &wire.route_templates.show, "run-profile")?,
        set: compile_literal_route(&base_url, &wire.route_templates.set, "run-profile")?,
        delete: compile_literal_route(&base_url, &wire.route_templates.delete, "run-profile")?,
        default: compile_literal_route(&base_url, &wire.route_templates.default, "run-profile")?,
        run: compile_literal_route(&base_url, &wire.route_templates.run, "run-profile")?,
    }))
}

#[derive(Clone)]
pub(in super::super) struct EnvironmentsDescriptor {
    base: Url,
    segments: Vec<String>,
}

impl EnvironmentsDescriptor {
    pub(in super::super) fn show(
        &self,
        scope: RunProfileScope,
        id: &EnvironmentId,
    ) -> Result<Url, TargetAuthorityError> {
        if matches!(id.as_str(), "." | "..") {
            return Err(authority_error("environment ID is not a path segment"));
        }
        let mut url = self.base.clone();
        let mut path = url
            .path_segments_mut()
            .map_err(|_| authority_error("environment base URL is invalid"))?;
        path.pop_if_empty();
        for segment in &self.segments {
            path.push(match segment.as_str() {
                "{scope}" => match scope {
                    RunProfileScope::User => "user",
                    RunProfileScope::Org => "org",
                },
                "{environment_id}" => id.as_str(),
                literal => literal,
            });
        }
        drop(path);
        Ok(url)
    }
}

pub(super) fn build_environments_descriptor(
    origin: &Url,
    extensions: &TargetDiscoveryExtensions,
) -> Result<Option<EnvironmentsDescriptor>, TargetAuthorityError> {
    let Some(wire) = &extensions.runtime_environments else {
        return Ok(None);
    };
    if wire.kind != RUNTIME_ENVIRONMENTS_KIND {
        return Err(authority_error("environment discovery is incompatible"));
    }
    validate_route_template(&wire.route_templates.show, "environment")?;
    let segments = wire
        .route_templates
        .show
        .split('/')
        .skip(1)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if segments.iter().filter(|s| s.as_str() == "{scope}").count() != 1
        || segments
            .iter()
            .filter(|s| s.as_str() == "{environment_id}")
            .count()
            != 1
        || segments.iter().any(|s| {
            !matches!(s.as_str(), "{scope}" | "{environment_id}") && !valid_literal_route_segment(s)
        })
    {
        return Err(authority_error("environment route template is invalid"));
    }
    Ok(Some(EnvironmentsDescriptor {
        base: capability_base_url(origin, &wire.base_url)?,
        segments,
    }))
}

#[cfg(test)]
mod tests;
