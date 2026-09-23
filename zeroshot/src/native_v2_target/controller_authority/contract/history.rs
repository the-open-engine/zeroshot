use openengine_cluster_protocol::{Cursor, RUN_HISTORY_KIND, RunId, TargetDiscoveryExtensions};
use reqwest::Url;

use super::{
    RunIdRouteSegment, authority_error, capability_base_url, compile_run_id_route_segments,
    validate_route_template,
};
use crate::native_v2_target::TargetAuthorityError;

#[derive(Clone)]
pub(crate) struct RunHistoryDescriptor {
    base_url: Url,
    list: HistoryRoute,
    detail: HistoryRoute,
    page: HistoryRoute,
}

#[derive(Clone)]
struct HistoryRoute {
    segments: Vec<RunIdRouteSegment>,
    after: bool,
}

impl RunHistoryDescriptor {
    pub(crate) fn list_url(&self, after: Option<&RunId>) -> Result<Url, TargetAuthorityError> {
        self.list
            .expand(&self.base_url, None, after.map(RunId::as_str))
    }

    pub(crate) fn detail_url(&self, run_id: &RunId) -> Result<Url, TargetAuthorityError> {
        self.detail.expand(&self.base_url, Some(run_id), None)
    }

    pub(crate) fn page_url(
        &self,
        run_id: &RunId,
        after: &Cursor,
    ) -> Result<Url, TargetAuthorityError> {
        self.page
            .expand(&self.base_url, Some(run_id), Some(after.as_str()))
    }
}

impl HistoryRoute {
    fn expand(
        &self,
        base_url: &Url,
        run_id: Option<&RunId>,
        after: Option<&str>,
    ) -> Result<Url, TargetAuthorityError> {
        let mut url = base_url.clone();
        let mut path = url
            .path_segments_mut()
            .map_err(|()| incompatible("base URL"))?;
        path.pop_if_empty();
        for segment in &self.segments {
            match segment {
                RunIdRouteSegment::Literal(segment) => path.push(segment),
                RunIdRouteSegment::RunId => path.push(
                    run_id
                        .ok_or_else(|| incompatible("route template"))?
                        .as_str(),
                ),
            };
        }
        drop(path);
        if self.after {
            if let Some(after) = after {
                url.query_pairs_mut().append_pair("after", after);
            }
        } else if after.is_some() {
            return Err(incompatible("route template"));
        }
        Ok(url)
    }
}

pub(crate) fn build_run_history_descriptor(
    origin: &Url,
    extensions: &TargetDiscoveryExtensions,
) -> Result<RunHistoryDescriptor, TargetAuthorityError> {
    let wire = extensions
        .run_history
        .as_ref()
        .ok_or_else(|| incompatible("capability"))?;
    if wire.kind != RUN_HISTORY_KIND {
        return Err(incompatible("capability kind"));
    }
    let base_url = capability_base_url(origin, &wire.base_url)?;
    Ok(RunHistoryDescriptor {
        base_url,
        list: compile_route(&wire.route_templates.list, false, true)?,
        detail: compile_route(&wire.route_templates.detail, true, false)?,
        page: compile_route(&wire.route_templates.page, true, true)?,
    })
}

fn compile_route(
    value: &str,
    requires_run_id: bool,
    allows_after: bool,
) -> Result<HistoryRoute, TargetAuthorityError> {
    validate_route_template(value, "run-history")?;
    let (path, after) = route_query(value)?;
    if after != allows_after {
        return Err(incompatible("route variables"));
    }
    let (segments, found_run_id) = compile_run_id_route_segments(path, "run-history")?;
    if found_run_id != requires_run_id {
        return Err(incompatible("route variables"));
    }
    Ok(HistoryRoute { segments, after })
}

fn route_query(value: &str) -> Result<(&str, bool), TargetAuthorityError> {
    if let Some(path) = value.strip_suffix("{?after}") {
        return Ok((path, true));
    }
    if value.contains('?') {
        return Err(incompatible("route template"));
    }
    Ok((value, false))
}

fn incompatible(subject: &str) -> TargetAuthorityError {
    authority_error(format!("run-history {subject} is incompatible"))
}

#[cfg(test)]
#[path = "history/tests.rs"]
mod tests;
