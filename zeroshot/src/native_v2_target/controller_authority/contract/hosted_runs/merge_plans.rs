use openengine_cluster_protocol::{MERGE_PLANS_KIND, MergePlanId, TargetDiscoveryExtensions};
use reqwest::Url;

use super::validate_route_template;
use crate::native_v2_target::controller_authority::contract::{
    authority_error, capability_base_url, valid_literal_route_segment,
};
use crate::native_v2_target::TargetAuthorityError;

#[derive(Clone)]
pub(crate) struct MergePlansDescriptor {
    base_url: Url,
    create: MergePlanRoute,
    status: MergePlanRoute,
    force: MergePlanRoute,
}

#[derive(Clone)]
struct MergePlanRoute {
    segments: Vec<MergePlanRouteSegment>,
}

#[derive(Clone)]
enum MergePlanRouteSegment {
    Literal(String),
    PlanId,
}

impl MergePlansDescriptor {
    pub(in super::super::super) fn create_url(&self) -> Result<Url, TargetAuthorityError> {
        self.create.expand(&self.base_url, None)
    }

    pub(in super::super::super) fn status_url(
        &self,
        plan_id: &MergePlanId,
    ) -> Result<Url, TargetAuthorityError> {
        self.status.expand(&self.base_url, Some(plan_id))
    }

    pub(in super::super::super) fn force_url(
        &self,
        plan_id: &MergePlanId,
    ) -> Result<Url, TargetAuthorityError> {
        self.force.expand(&self.base_url, Some(plan_id))
    }
}

impl MergePlanRoute {
    fn expand(
        &self,
        base_url: &Url,
        plan_id: Option<&MergePlanId>,
    ) -> Result<Url, TargetAuthorityError> {
        let mut url = base_url.clone();
        let mut path = url
            .path_segments_mut()
            .map_err(|_| authority_error("merge-plan base URL is invalid"))?;
        path.pop_if_empty();
        for segment in &self.segments {
            match segment {
                MergePlanRouteSegment::Literal(value) => path.push(value),
                MergePlanRouteSegment::PlanId => path.push(
                    plan_id
                        .ok_or_else(|| authority_error("merge-plan route is incomplete"))?
                        .as_str(),
                ),
            };
        }
        drop(path);
        Ok(url)
    }
}

pub(crate) fn build_merge_plans_descriptor(
    origin: &Url,
    extensions: &TargetDiscoveryExtensions,
) -> Result<Option<MergePlansDescriptor>, TargetAuthorityError> {
    let Some(wire) = &extensions.merge_plans else {
        return Ok(None);
    };
    if wire.kind != MERGE_PLANS_KIND {
        return Err(authority_error("merge-plan discovery is incompatible"));
    }
    let base_url = capability_base_url(origin, &wire.base_url)?;
    Ok(Some(MergePlansDescriptor {
        base_url,
        create: compile_route(&wire.route_templates.create, false)?,
        status: compile_route(&wire.route_templates.status, true)?,
        force: compile_route(&wire.route_templates.force, true)?,
    }))
}

fn compile_route(
    value: &str,
    requires_plan_id: bool,
) -> Result<MergePlanRoute, TargetAuthorityError> {
    validate_route_template(value)?;
    let mut found_plan_id = false;
    let mut segments = Vec::new();
    for segment in value.split('/').skip(1) {
        if segment == "{plan_id}" {
            if found_plan_id {
                return Err(unsupported_variables());
            }
            found_plan_id = true;
            segments.push(MergePlanRouteSegment::PlanId);
        } else if valid_literal_route_segment(segment) {
            segments.push(MergePlanRouteSegment::Literal(segment.to_owned()));
        } else {
            return Err(authority_error("merge-plan route template is invalid"));
        }
    }
    if found_plan_id != requires_plan_id {
        return Err(unsupported_variables());
    }
    Ok(MergePlanRoute { segments })
}

fn unsupported_variables() -> TargetAuthorityError {
    authority_error("merge-plan route template declares unsupported variables")
}
