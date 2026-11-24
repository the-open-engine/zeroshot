use async_trait::async_trait;
use openengine_cluster_protocol as protocol;
use openengine_cluster_protocol::{
    HostedRunStreamFrame, RunForceParams, RunListParams, RunLogEventNotification, RunLogsParams,
    RunStatusParams, RunWatchParams,
};
use reqwest::header::{ACCEPT, CACHE_CONTROL};
use reqwest::{RequestBuilder, Response, Url};
use serde::de::DeserializeOwned;

use super::contract::{
    HostedRunsDescriptor, StacksDescriptor, authority_error, http_error, read_success_json,
    read_success_json_with_limit, require_response_route,
};
use super::TargetHttpControlAuthority;
use super::access::AccessToken;
use zeroshot_engine::native_v2_cli::oecp::{BoxedSubscription, HostedRunRoute};
use zeroshot_engine::native_v2_cli::{
    CliRunForceResult, CliRunListResult, CliRunWatchEventNotification, CliSubscription,
    CliSubscriptionItem, NativeV2CliError,
};
use crate::native_v2_target::{TargetAccess, TargetAuthorityError, TargetRecord};

const NDJSON_MEDIA_TYPE: &str = "application/x-ndjson";
const MAX_STREAM_FRAME_BYTES: usize = 64 * 1024;

#[path = "hosted_runs/export_http.rs"]
mod export_http;
#[path = "hosted_runs/list_http.rs"]
mod list_http;
#[path = "hosted_runs/part_identity.rs"]
pub(super) mod part_identity;
#[path = "hosted_runs/recovery_http.rs"]
mod recovery_http;
#[path = "hosted_runs/stacks_http.rs"]
mod stacks_http;

pub(super) struct HostedRunResponse {
    operation: &'static str,
    maximum_bytes: Option<usize>,
    method: &'static str,
}

impl HostedRunResponse {
    pub(super) const fn new(operation: &'static str, method: &'static str) -> Self {
        Self {
            operation,
            maximum_bytes: None,
            method,
        }
    }

    const fn with_limit(mut self, maximum_bytes: usize) -> Self {
        self.maximum_bytes = Some(maximum_bytes);
        self
    }
}

struct HostedRunSubscription<E> {
    response: Response,
    buffered: Vec<u8>,
    maximum_frame_bytes: usize,
    run_parts: bool,
    closed: bool,
    marker: std::marker::PhantomData<E>,
}

impl<E> HostedRunSubscription<E> {
    const fn new(response: Response, maximum_frame_bytes: usize, run_parts: bool) -> Self {
        Self {
            response,
            buffered: Vec::new(),
            run_parts,
            maximum_frame_bytes,
            closed: false,
            marker: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<E> CliSubscription<E> for HostedRunSubscription<E>
where
    E: DeserializeOwned + Send,
{
    async fn next(&mut self) -> Result<Option<CliSubscriptionItem<E>>, NativeV2CliError> {
        if self.closed {
            return Ok(None);
        }
        loop {
            if let Some(line) = self.take_line() {
                return self.decode(&line).map(Some);
            }
            if self.buffered.len() > self.maximum_frame_bytes {
                return Err(stream_error("hosted run stream frame is too large"));
            }
            if !self.read_more().await? {
                return Ok(None);
            }
        }
    }
}

impl<E> HostedRunSubscription<E>
where
    E: DeserializeOwned,
{
    fn take_line(&mut self) -> Option<Vec<u8>> {
        let line_end = self.buffered.iter().position(|byte| *byte == b'\n')?;
        let mut line = self.buffered.drain(..=line_end).collect::<Vec<_>>();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Some(line)
    }

    async fn read_more(&mut self) -> Result<bool, NativeV2CliError> {
        let chunk = self
            .response
            .chunk()
            .await
            .map_err(|_| NativeV2CliError::Disconnected)?;
        match chunk {
            Some(chunk) => {
                self.buffered.extend_from_slice(&chunk);
                Ok(true)
            }
            None if self.buffered.is_empty() => Ok(false),
            None => Err(NativeV2CliError::Disconnected),
        }
    }

    fn decode(&mut self, line: &[u8]) -> Result<CliSubscriptionItem<E>, NativeV2CliError> {
        if line.is_empty() || line.len() > self.maximum_frame_bytes {
            return Err(stream_error("hosted run stream frame is malformed"));
        }
        match part_identity::decode::<HostedRunStreamFrame<E>>(line, self.run_parts, |frame| {
            if let Some(event) = frame.get_mut("event") {
                protocol::physical_runs::read_response("event", event);
            }
        })
        .map_err(|_| stream_error("hosted run stream frame is malformed"))?
        {
            HostedRunStreamFrame::Event { event } => Ok(CliSubscriptionItem::Event(event)),
            HostedRunStreamFrame::Closed { reason } => {
                self.closed = true;
                Ok(CliSubscriptionItem::Closed { reason })
            }
        }
    }
}

impl TargetHttpControlAuthority {
    async fn require_hosted_run_access(
        &self,
        target: &TargetRecord,
    ) -> Result<(HostedRunsDescriptor, AccessToken), TargetAuthorityError> {
        self.require_hosted_run_access_for(target, false).await
    }

    async fn require_hosted_run_access_for(
        &self,
        target: &TargetRecord,
        git_transport: bool,
    ) -> Result<(HostedRunsDescriptor, AccessToken), TargetAuthorityError> {
        if matches!(target.access, TargetAccess::Direct) {
            return Err(TargetAuthorityError::new(
                "direct target does not use hosted run lifecycle",
            ));
        }
        let (auth, controller) = self.descriptors(target).await?;
        let mut routes = auth.hosted_runs.clone();
        routes.run_parts = controller.run_parts;
        super::require_git_transport(&controller, git_transport)?;
        let access = self
            .access_token(target, &auth, &controller.audience)
            .await?;
        Ok((routes, access))
    }

    pub(super) async fn hosted_json<T: DeserializeOwned>(
        &self,
        request: (RequestBuilder, &AccessToken),
        operation: &'static str,
        maximum_bytes: Option<usize>,
    ) -> Result<T, TargetAuthorityError> {
        let (request, access) = request;
        let request = request.build().map_err(|_| {
            TargetAuthorityError::disconnected(format!("{operation} request failed"))
        })?;
        let expected = request.url().clone();
        let response = self
            .client
            .execute(request)
            .await
            .map_err(|error| TargetAuthorityError::request_failed(operation, &error))?;
        require_response_route(&response, &expected)?;
        self.invalidate_rejected_access(&response, Some(access))
            .await;
        if let Some(maximum_bytes) = maximum_bytes {
            read_success_json_with_limit(response, &expected, operation, maximum_bytes).await
        } else {
            read_success_json(response, &expected, operation).await
        }
    }

    pub(super) async fn hosted_run_json<T: DeserializeOwned>(
        &self,
        request: (RequestBuilder, &AccessToken),
        run_parts: bool,
        response: HostedRunResponse,
    ) -> Result<T, TargetAuthorityError> {
        let HostedRunResponse {
            operation,
            maximum_bytes,
            method,
        } = response;
        if run_parts {
            let (builder, access) = request;
            let builder = part_identity::negotiate(builder, run_parts);
            return self
                .hosted_json((builder, access), operation, maximum_bytes)
                .await;
        }
        let mut value = self.hosted_json(request, operation, maximum_bytes).await?;
        protocol::physical_runs::read_response(method, &mut value);
        serde_json::from_value(value)
            .map_err(|_| authority_error(format!("{operation} response is malformed")))
    }

    async fn hosted_stream<E>(
        &self,
        request: (Url, &AccessToken, bool),
        operation: &'static str,
        maximum_frame_bytes: usize,
    ) -> Result<BoxedSubscription<E>, TargetAuthorityError>
    where
        E: DeserializeOwned + Send + 'static,
    {
        let (url, access, run_parts) = request;
        let builder = part_identity::negotiate(self.stream_client.get(url.clone()), run_parts);
        let response = self
            .authorized(builder, access)?
            .header(ACCEPT, NDJSON_MEDIA_TYPE)
            .header(CACHE_CONTROL, "no-store")
            .send()
            .await
            .map_err(|error| TargetAuthorityError::request_failed(operation, &error))?;
        require_response_route(&response, &url)?;
        self.invalidate_rejected_access(&response, Some(access))
            .await;
        if !response.status().is_success() {
            return Err(http_error(response, operation).await);
        }
        Ok(BoxedSubscription::new(HostedRunSubscription::new(
            response,
            maximum_frame_bytes,
            run_parts,
        )))
    }
}

impl TargetHttpControlAuthority {
    pub(super) async fn hosted_run_list(
        &self,
        target: &TargetRecord,
        _params: RunListParams,
    ) -> Result<CliRunListResult, TargetAuthorityError> {
        let (routes, access) = self.require_hosted_run_access(target).await?;
        let url = routes.list_url()?;
        let builder = part_identity::negotiate(self.client.get(url.clone()), routes.run_parts);
        let response = self
            .authorized(builder, &access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .send()
            .await
            .map_err(|error| TargetAuthorityError::request_failed("hosted run list", &error))?;
        require_response_route(&response, &url)?;
        self.invalidate_rejected_access(&response, Some(&access))
            .await;
        if !response.status().is_success() {
            return Err(http_error(response, "hosted run list").await);
        }
        list_http::read_run_list(response, routes.run_parts).await
    }

    pub(super) async fn hosted_run_status(
        &self,
        target: &TargetRecord,
        params: RunStatusParams,
        grouped_only: bool,
    ) -> Result<Option<HostedRunRoute>, TargetAuthorityError> {
        let (routes, access) = self.require_hosted_run_access(target).await?;
        if grouped_only && !routes.run_parts {
            return Ok(None);
        }
        let url = routes.status_url(&params.run_id)?;
        let url = part_identity::selected_url(
            url,
            routes.run_parts,
            &params.run_id,
            params.part_id.as_ref(),
        )?;
        let request = self
            .authorized(self.client.get(url), &access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store");
        let response = HostedRunResponse::new("hosted run status", protocol::RUN_STATUS_METHOD)
            .with_limit(openengine_cluster_protocol::MAX_REQUEST_RESPONSE_BYTES);
        let status = self
            .hosted_run_json((request, &access), routes.run_parts, response)
            .await?;
        Ok(Some(HostedRunRoute {
            status,
            run_parts: routes.run_parts,
        }))
    }

    pub(super) async fn hosted_run_watch(
        &self,
        target: &TargetRecord,
        params: RunWatchParams,
    ) -> Result<BoxedSubscription<CliRunWatchEventNotification>, TargetAuthorityError> {
        let (routes, access) = self.require_hosted_run_access(target).await?;
        let url = routes.watch_url(&params.run_id, params.from_cursor.as_ref())?;
        let url = part_identity::selected_url(
            url,
            routes.run_parts,
            &params.run_id,
            params.part_id.as_ref(),
        )?;
        self.hosted_stream(
            (url, &access, routes.run_parts),
            "hosted run watch",
            openengine_cluster_protocol::MAX_REQUEST_RESPONSE_BYTES,
        )
        .await
    }

    pub(super) async fn hosted_run_logs(
        &self,
        target: &TargetRecord,
        params: RunLogsParams,
    ) -> Result<BoxedSubscription<RunLogEventNotification>, TargetAuthorityError> {
        let (routes, access) = self.require_hosted_run_access(target).await?;
        let url = routes.logs_url(
            &params.run_id,
            params.from_cursor.as_ref(),
            params.execution.as_ref(),
        )?;
        let url = part_identity::selected_url(
            url,
            routes.run_parts,
            &params.run_id,
            params.part_id.as_ref(),
        )?;
        self.hosted_stream(
            (url, &access, routes.run_parts),
            "hosted run logs",
            MAX_STREAM_FRAME_BYTES,
        )
        .await
    }

    pub(super) async fn hosted_run_force(
        &self,
        target: &TargetRecord,
        params: RunForceParams,
    ) -> Result<CliRunForceResult, TargetAuthorityError> {
        let (routes, access) = self.require_hosted_run_access(target).await?;
        let url = routes.force_url(&params.run_id)?;
        let mut body = serde_json::Map::new();
        if let Some(part) = params.part_id.as_ref() {
            part_identity::validate_part(routes.run_parts, &params.run_id, part)?;
            if routes.run_parts {
                body.insert("partId".into(), serde_json::json!(part));
            }
        }
        let request = self
            .authorized(self.client.post(url.clone()), &access)?
            .header(ACCEPT, "application/json")
            .json(&body);
        self.hosted_run_json(
            (request, &access),
            routes.run_parts,
            HostedRunResponse::new("hosted run force", protocol::RUN_FORCE_METHOD),
        )
        .await
    }
}

fn stream_error(message: &'static str) -> NativeV2CliError {
    NativeV2CliError::Target(message.to_owned())
}
