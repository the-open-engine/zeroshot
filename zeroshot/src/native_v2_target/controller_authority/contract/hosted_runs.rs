use openengine_cluster_protocol::{
    Cursor, ExecutionRef, HOSTED_WORKSPACE_RECOVERY_KIND, RunId, TargetDiscoveryExtensions,
    TargetHostedRunRoutes, TargetHostedWorkspaceRecoveryRoutes,
};
use reqwest::Url;

#[path = "hosted_runs/merge_plans.rs"]
mod merge_plans;
pub(crate) use merge_plans::{MergePlansDescriptor, build_merge_plans_descriptor};

use super::{
    RunIdRouteSegment, authority_error, capability_base_url, compile_run_id_route_segments,
    validate_route_template,
};
use crate::native_v2_target::TargetAuthorityError;

const HOSTED_RUNS_KIND: &str = "zeroshot.hosted-runs/v1";

#[derive(Clone)]
pub(in super::super) struct HostedRunsDescriptor {
    base_url: Url,
    list: HostedRunRoute,
    status: HostedRunRoute,
    watch: HostedRunRoute,
    logs: HostedRunRoute,
    force: HostedRunRoute,
    resume: Option<HostedRunRoute>,
    checkpoints: Option<HostedRunRoute>,
    discard_workspace: Option<HostedRunRoute>,
}

#[derive(Clone)]
struct HostedRunRoute {
    segments: Vec<RunIdRouteSegment>,
    query: Vec<HostedRunQuery>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum HostedRunQuery {
    FromCursor,
    Execution,
}

#[derive(Clone, Copy, Default)]
struct RouteValues<'a> {
    run_id: Option<&'a RunId>,
    from_cursor: Option<&'a Cursor>,
    execution: Option<&'a ExecutionRef>,
}

impl HostedRunsDescriptor {
    pub(in super::super) fn resume_url(&self, run_id: &RunId) -> Result<Url, TargetAuthorityError> {
        self.recovery_url(self.resume.as_ref(), run_id)
    }

    pub(in super::super) fn checkpoints_url(
        &self,
        run_id: &RunId,
    ) -> Result<Url, TargetAuthorityError> {
        self.recovery_url(self.checkpoints.as_ref(), run_id)
    }

    pub(in super::super) fn discard_workspace_url(
        &self,
        run_id: &RunId,
    ) -> Result<Url, TargetAuthorityError> {
        self.recovery_url(self.discard_workspace.as_ref(), run_id)
    }

    fn recovery_url(
        &self,
        route: Option<&HostedRunRoute>,
        run_id: &RunId,
    ) -> Result<Url, TargetAuthorityError> {
        route
            .ok_or_else(|| authority_error("target does not advertise hosted workspace recovery"))?
            .expand(
                &self.base_url,
                RouteValues {
                    run_id: Some(run_id),
                    ..RouteValues::default()
                },
            )
    }

    pub(in super::super) fn list_url(&self) -> Result<Url, TargetAuthorityError> {
        self.list.expand(&self.base_url, RouteValues::default())
    }

    pub(in super::super) fn status_url(&self, run_id: &RunId) -> Result<Url, TargetAuthorityError> {
        self.status.expand(
            &self.base_url,
            RouteValues {
                run_id: Some(run_id),
                ..RouteValues::default()
            },
        )
    }

    pub(in super::super) fn watch_url(
        &self,
        run_id: &RunId,
        from_cursor: Option<&Cursor>,
    ) -> Result<Url, TargetAuthorityError> {
        self.watch.expand(
            &self.base_url,
            RouteValues {
                run_id: Some(run_id),
                from_cursor,
                execution: None,
            },
        )
    }

    pub(in super::super) fn logs_url(
        &self,
        run_id: &RunId,
        from_cursor: Option<&Cursor>,
        execution: Option<&ExecutionRef>,
    ) -> Result<Url, TargetAuthorityError> {
        self.logs.expand(
            &self.base_url,
            RouteValues {
                run_id: Some(run_id),
                from_cursor,
                execution,
            },
        )
    }

    pub(in super::super) fn force_url(&self, run_id: &RunId) -> Result<Url, TargetAuthorityError> {
        self.force.expand(
            &self.base_url,
            RouteValues {
                run_id: Some(run_id),
                ..RouteValues::default()
            },
        )
    }
}

impl HostedRunRoute {
    fn expand(&self, base_url: &Url, values: RouteValues<'_>) -> Result<Url, TargetAuthorityError> {
        let mut url = base_url.clone();
        self.append_path(&mut url, values.run_id)?;
        self.append_query(&mut url, values);
        Ok(url)
    }

    fn append_path(
        &self,
        url: &mut Url,
        run_id: Option<&RunId>,
    ) -> Result<(), TargetAuthorityError> {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| authority_error("hosted run base URL is invalid"))?;
        path.pop_if_empty();
        for segment in &self.segments {
            let value = match segment {
                RunIdRouteSegment::Literal(segment) => segment.as_str(),
                RunIdRouteSegment::RunId => run_id
                    .ok_or_else(|| authority_error("hosted run route is incomplete"))?
                    .as_str(),
            };
            path.push(value);
        }
        Ok(())
    }

    fn append_query(&self, url: &mut Url, values: RouteValues<'_>) {
        if values.from_cursor.is_none() && values.execution.is_none() {
            return;
        }
        let mut query = url.query_pairs_mut();
        for variable in &self.query {
            match variable {
                HostedRunQuery::FromCursor => values.from_cursor.map(|cursor| {
                    query.append_pair("from_cursor", cursor.as_str());
                }),
                HostedRunQuery::Execution => values.execution.map(|execution| {
                    query.append_pair("execution", execution.as_str());
                }),
            };
        }
    }
}

pub(super) fn build_hosted_runs_descriptor(
    origin: &Url,
    extensions: &TargetDiscoveryExtensions,
) -> Result<HostedRunsDescriptor, TargetAuthorityError> {
    let wire = extensions.hosted_runs.as_ref().ok_or_else(|| {
        authority_error("hosted target does not advertise zeroshot.hosted-runs/v1")
    })?;
    if wire.kind != HOSTED_RUNS_KIND {
        return Err(authority_error("hosted run discovery is incompatible"));
    }
    let base_url = capability_base_url(origin, &wire.base_url)?;
    let mut descriptor = compile_hosted_run_routes(base_url, &wire.route_templates)?;
    if let Some(recovery) = extensions.hosted_workspace_recovery.as_ref() {
        if recovery.kind != HOSTED_WORKSPACE_RECOVERY_KIND {
            return Err(authority_error(
                "hosted workspace recovery discovery is incompatible",
            ));
        }
        compile_recovery_routes(&mut descriptor, &recovery.route_templates)?;
    }
    Ok(descriptor)
}

fn compile_hosted_run_routes(
    base_url: Url,
    routes: &TargetHostedRunRoutes,
) -> Result<HostedRunsDescriptor, TargetAuthorityError> {
    Ok(HostedRunsDescriptor {
        base_url,
        list: compile_hosted_run_route(&routes.list, false, &[])?,
        status: compile_hosted_run_route(&routes.status, true, &[])?,
        watch: compile_hosted_run_route(&routes.watch, true, &[HostedRunQuery::FromCursor])?,
        logs: compile_hosted_run_route(
            &routes.logs,
            true,
            &[HostedRunQuery::FromCursor, HostedRunQuery::Execution],
        )?,
        force: compile_hosted_run_route(&routes.force, true, &[])?,
        resume: None,
        checkpoints: None,
        discard_workspace: None,
    })
}

fn compile_recovery_routes(
    descriptor: &mut HostedRunsDescriptor,
    routes: &TargetHostedWorkspaceRecoveryRoutes,
) -> Result<(), TargetAuthorityError> {
    descriptor.resume = Some(compile_hosted_run_route(&routes.resume, true, &[])?);
    descriptor.checkpoints = Some(compile_hosted_run_route(&routes.checkpoints, true, &[])?);
    descriptor.discard_workspace = Some(compile_hosted_run_route(
        &routes.discard_workspace,
        true,
        &[],
    )?);
    Ok(())
}

fn compile_hosted_run_route(
    value: &str,
    requires_run_id: bool,
    expected_query: &[HostedRunQuery],
) -> Result<HostedRunRoute, TargetAuthorityError> {
    validate_route_template(value, "hosted run")?;
    let (path, query) = split_route_query(value)?;
    if query != expected_query {
        return Err(unsupported_variables());
    }
    let (segments, found_run_id) = compile_run_id_route_segments(path, "hosted run")?;
    if found_run_id != requires_run_id {
        return Err(unsupported_variables());
    }
    Ok(HostedRunRoute {
        segments,
        query: query.to_vec(),
    })
}

fn unsupported_variables() -> TargetAuthorityError {
    authority_error("hosted run route template declares unsupported variables")
}

fn split_route_query(value: &str) -> Result<(&str, Vec<HostedRunQuery>), TargetAuthorityError> {
    if let Some(path) = value.strip_suffix("{?from_cursor}") {
        return Ok((path, vec![HostedRunQuery::FromCursor]));
    }
    if let Some(path) = value.strip_suffix("{?from_cursor,execution}") {
        return Ok((
            path,
            vec![HostedRunQuery::FromCursor, HostedRunQuery::Execution],
        ));
    }
    if value.contains('?') {
        return Err(authority_error("hosted run route template is invalid"));
    }
    Ok((value, Vec::new()))
}

#[cfg(test)]
#[path = "hosted_runs/tests.rs"]
mod tests;
