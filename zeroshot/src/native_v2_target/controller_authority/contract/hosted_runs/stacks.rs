use openengine_cluster_protocol::{STACKS_KIND, StackId, TargetDiscoveryExtensions};
use reqwest::Url;

use crate::native_v2_target::controller_authority::contract::{
    authority_error, capability_base_url, valid_literal_route_segment, validate_route_template,
};
use crate::native_v2_target::TargetAuthorityError;

#[derive(Clone)]
pub(crate) struct StacksDescriptor {
    base_url: Url,
    create: StackRoute,
    status: StackRoute,
    force: StackRoute,
}

#[derive(Clone)]
struct StackRoute {
    segments: Vec<StackRouteSegment>,
}

#[derive(Clone)]
enum StackRouteSegment {
    Literal(String),
    StackId,
}

impl StacksDescriptor {
    pub(in super::super::super) fn create_url(&self) -> Result<Url, TargetAuthorityError> {
        self.create.expand(&self.base_url, None)
    }

    pub(in super::super::super) fn status_url(
        &self,
        stack_id: &StackId,
    ) -> Result<Url, TargetAuthorityError> {
        self.status.expand(&self.base_url, Some(stack_id))
    }

    pub(in super::super::super) fn force_url(
        &self,
        stack_id: &StackId,
    ) -> Result<Url, TargetAuthorityError> {
        self.force.expand(&self.base_url, Some(stack_id))
    }
}

impl StackRoute {
    fn expand(
        &self,
        base_url: &Url,
        stack_id: Option<&StackId>,
    ) -> Result<Url, TargetAuthorityError> {
        let mut url = base_url.clone();
        let mut path = url
            .path_segments_mut()
            .map_err(|_| authority_error("stack base URL is invalid"))?;
        path.pop_if_empty();
        for segment in &self.segments {
            match segment {
                StackRouteSegment::Literal(value) => path.push(value),
                StackRouteSegment::StackId => path.push(
                    stack_id
                        .ok_or_else(|| authority_error("stack route is incomplete"))?
                        .as_str(),
                ),
            };
        }
        drop(path);
        Ok(url)
    }
}

pub(crate) fn build_stacks_descriptor(
    origin: &Url,
    extensions: &TargetDiscoveryExtensions,
) -> Result<Option<StacksDescriptor>, TargetAuthorityError> {
    let Some(wire) = &extensions.stacks else {
        return Ok(None);
    };
    if wire.kind != STACKS_KIND {
        return Err(authority_error("stack discovery is incompatible"));
    }
    let base_url = capability_base_url(origin, &wire.base_url)?;
    Ok(Some(StacksDescriptor {
        base_url,
        create: compile_route(&wire.route_templates.create, false)?,
        status: compile_route(&wire.route_templates.status, true)?,
        force: compile_route(&wire.route_templates.force, true)?,
    }))
}

fn compile_route(value: &str, requires_stack_id: bool) -> Result<StackRoute, TargetAuthorityError> {
    validate_route_template(value, "stack")?;
    let mut found_stack_id = false;
    let mut segments = Vec::new();
    for segment in value.split('/').skip(1) {
        if segment == "{stack_id}" {
            if found_stack_id {
                return Err(unsupported_variables());
            }
            found_stack_id = true;
            segments.push(StackRouteSegment::StackId);
        } else if valid_literal_route_segment(segment) {
            segments.push(StackRouteSegment::Literal(segment.to_owned()));
        } else {
            return Err(authority_error("stack route template is invalid"));
        }
    }
    if found_stack_id != requires_stack_id {
        return Err(unsupported_variables());
    }
    Ok(StackRoute { segments })
}

fn unsupported_variables() -> TargetAuthorityError {
    authority_error("stack route template declares unsupported variables")
}

#[cfg(test)]
#[path = "stacks/tests.rs"]
mod tests;
