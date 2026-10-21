use openengine_cluster_protocol::DelegationOwner;
use reqwest::{
    Method, RequestBuilder, Response, Url,
    header::{ACCEPT, CACHE_CONTROL},
};
use serde::Deserialize;
use serde_json::Value;
use zeroshot_engine::native_v2_cli::{DelegationAction, DelegationResult};

use super::super::{ADMIN_AUDIENCE, HostedAuthDescriptor, TargetHttpControlAuthority};
use super::super::contract::{
    authority_error, http_error, read_success_json, require_response_route, same_origin_path,
};
use crate::native_v2_target::{TargetAccess, TargetAuthorityError, TargetRecord};

#[derive(Deserialize)]
struct AdminIdentity {
    id: String,
}

impl TargetHttpControlAuthority {
    pub(in super::super) async fn verify_admin_session(
        &self,
        auth: &HostedAuthDescriptor,
        access_token: &str,
    ) -> Result<(), TargetAuthorityError> {
        let url = same_origin_path(&auth.metadata_url, "/auth/v1/me")?;
        let response = self
            .authorized(self.client.get(url.clone()), access_token)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .send()
            .await
            .map_err(|error| {
                TargetAuthorityError::request_failed("account verification", &error)
            })?;
        let identity: AdminIdentity =
            read_success_json(response, &url, "account verification").await?;
        if uuid::Uuid::parse_str(&identity.id).is_err() {
            return Err(authority_error(
                "account verification response is malformed",
            ));
        }
        Ok(())
    }

    pub(in super::super) async fn manage_delegation(
        &self,
        target: &TargetRecord,
        action: DelegationAction,
    ) -> Result<DelegationResult, TargetAuthorityError> {
        self.require_human_credential()?;
        if matches!(target.access, TargetAccess::Direct) {
            return Err(authority_error(
                "delegation management requires a hosted target",
            ));
        }
        let (auth, _) = self.descriptors(target).await?;
        let base = auth
            .delegations
            .as_ref()
            .ok_or_else(|| authority_error("target does not advertise delegation management"))?;
        let access = self.access_token(target, &auth, ADMIN_AUDIENCE).await?;
        let request = self.delegation_request(base, &action)?;
        let (expected, response) = self.send_delegation_request(request, &access).await?;
        self.invalidate_rejected_access(&response, Some(&access))
            .await;
        read_delegation_response(action, response, &expected).await
    }

    async fn send_delegation_request(
        &self,
        request: RequestBuilder,
        access: &str,
    ) -> Result<(Url, Response), TargetAuthorityError> {
        let request = self
            .authorized(request, access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .build()
            .map_err(|_| authority_error("delegation request is malformed"))?;
        let expected = request.url().clone();
        let response = self.client.execute(request).await.map_err(|error| {
            TargetAuthorityError::request_failed("delegation management", &error)
        })?;
        require_response_route(&response, &expected)?;
        Ok((expected, response))
    }

    fn delegation_request(
        &self,
        base: &Url,
        action: &DelegationAction,
    ) -> Result<RequestBuilder, TargetAuthorityError> {
        let (method, mut url) = operation_route(base, action)?;
        if let DelegationAction::List {
            owner,
            organization_id,
        } = action
        {
            url.query_pairs_mut().append_pair(
                "owner",
                match owner {
                    DelegationOwner::User => "user",
                    DelegationOwner::Org => "org",
                },
            );
            if let Some(organization) = organization_id {
                url.query_pairs_mut()
                    .append_pair("organization_id", organization);
            }
        }
        let request = self.client.request(method, url);
        Ok(match action {
            DelegationAction::Create(body) => request.json(body),
            DelegationAction::KeyCreate { request: body, .. } => request.json(body),
            _ => request,
        })
    }
}

fn operation_route(
    base: &Url,
    action: &DelegationAction,
) -> Result<(Method, Url), TargetAuthorityError> {
    let (method, segments) = match action {
        DelegationAction::List { .. } => (Method::GET, Vec::new()),
        DelegationAction::Create(_) => (Method::POST, Vec::new()),
        DelegationAction::Show { id } => (Method::GET, vec![id.as_str()]),
        DelegationAction::Disable { id } => (Method::DELETE, vec![id.as_str()]),
        DelegationAction::KeyList { id } => (Method::GET, vec![id.as_str(), "keys"]),
        DelegationAction::KeyCreate { id, .. } => (Method::POST, vec![id.as_str(), "keys"]),
        DelegationAction::KeyRevoke { id, key_id } => {
            (Method::DELETE, vec![id.as_str(), "keys", key_id.as_str()])
        }
    };
    Ok((method, append_segments(base, segments)?))
}

fn append_segments(base: &Url, segments: Vec<&str>) -> Result<Url, TargetAuthorityError> {
    if segments
        .iter()
        .any(|segment| *segment != "keys" && uuid::Uuid::parse_str(segment).is_err())
    {
        return Err(authority_error("delegation and key IDs must be UUIDs"));
    }
    let mut url = base.clone();
    if !segments.is_empty() {
        url.path_segments_mut()
            .map_err(|_| authority_error("delegation route is malformed"))?
            .pop_if_empty()
            .extend(segments);
    }
    Ok(url)
}

async fn read_delegation_response(
    action: DelegationAction,
    response: Response,
    expected: &Url,
) -> Result<DelegationResult, TargetAuthorityError> {
    if !response.status().is_success() {
        return Err(http_error(response, "delegation management").await);
    }
    match action {
        DelegationAction::Disable { .. } => Ok(DelegationResult::Disabled { disabled: true }),
        DelegationAction::KeyRevoke { .. } => Ok(DelegationResult::Revoked { revoked: true }),
        action => {
            let value: Value =
                read_success_json(response, expected, "delegation management").await?;
            decode_result(action, value)
        }
    }
}

fn decode_result(
    action: DelegationAction,
    value: Value,
) -> Result<DelegationResult, TargetAuthorityError> {
    let result = match action {
        DelegationAction::List { .. } => serde_json::from_value(value).map(DelegationResult::List),
        DelegationAction::Create(_) => serde_json::from_value(value).map(DelegationResult::Created),
        DelegationAction::Show { .. } => serde_json::from_value(value).map(DelegationResult::Show),
        DelegationAction::KeyList { .. } => {
            serde_json::from_value(value).map(DelegationResult::KeyList)
        }
        DelegationAction::KeyCreate { .. } => {
            serde_json::from_value(value).map(DelegationResult::KeyCreated)
        }
        _ => return Err(authority_error("unsupported delegation response")),
    };
    result.map_err(|_| authority_error("delegation response is malformed"))
}
