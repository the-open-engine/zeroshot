use std::ops::Deref;
use std::sync::Arc;
use std::time::Instant;

use reqwest::header::{HeaderMap, HeaderValue, WWW_AUTHENTICATE};
use reqwest::{Response, StatusCode, Url};

use super::{
    HostedAuthDescriptor, IssuedAccessToken, MergePlansDescriptor, TargetAuthorityError,
    TargetHttpControlAuthority, TargetRecord,
};

#[derive(Clone)]
pub(super) struct AccessToken(Arc<IssuedAccessToken>);

pub(super) struct HostedAccess<'a> {
    pub(super) target: &'a TargetRecord,
    pub(super) auth: &'a HostedAuthDescriptor,
    pub(super) audience: &'a str,
}

impl Deref for AccessToken {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0.value
    }
}

#[derive(PartialEq, Eq)]
struct AccessIdentity {
    target: TargetRecord,
    metadata_url: Url,
    token_endpoint: Url,
    session_endpoint: Url,
    client_id: String,
    audience: String,
}

impl AccessIdentity {
    fn new(target: &TargetRecord, auth: &HostedAuthDescriptor, audience: &str) -> Self {
        Self {
            target: target.clone(),
            metadata_url: auth.metadata_url.clone(),
            token_endpoint: auth.token_endpoint.clone(),
            session_endpoint: auth.session_endpoint.clone(),
            client_id: auth.client_id.clone(),
            audience: audience.to_owned(),
        }
    }
}

pub(super) struct CachedHostedAccess {
    identity: AccessIdentity,
    access: AccessToken,
    merge_plans: Option<MergePlansDescriptor>,
}

impl CachedHostedAccess {
    pub(super) fn merge_plan_access(
        &self,
        target: &TargetRecord,
    ) -> Option<(MergePlansDescriptor, AccessToken)> {
        (self.identity.target == *target && Instant::now() < self.access.0.reusable_until)
            .then(|| {
                self.merge_plans
                    .clone()
                    .map(|routes| (routes, self.access.clone()))
            })
            .flatten()
    }
}

impl TargetHttpControlAuthority {
    pub(super) async fn access_token(
        &self,
        target: &TargetRecord,
        auth: &HostedAuthDescriptor,
        audience: &str,
    ) -> Result<AccessToken, TargetAuthorityError> {
        let cached = Arc::clone(&self.hosted_access).lock_owned().await;
        let authority = self.clone();
        let target = target.clone();
        let auth = auth.clone();
        let audience = audience.to_owned();
        tokio::spawn(async move {
            let mut cached = cached;
            authority
                .access_token_locked(
                    &mut cached,
                    HostedAccess {
                        target: &target,
                        auth: &auth,
                        audience: &audience,
                    },
                )
                .await
        })
        .await
        .map_err(|_| TargetAuthorityError::new("target access task failed"))?
    }

    pub(super) async fn access_token_locked(
        &self,
        cached: &mut Option<CachedHostedAccess>,
        request: HostedAccess<'_>,
    ) -> Result<AccessToken, TargetAuthorityError> {
        let HostedAccess {
            target,
            auth,
            audience,
        } = request;
        let identity = AccessIdentity::new(target, auth, audience);
        if let Some(entry) = cached.as_mut()
            && entry.identity == identity
            && Instant::now() < entry.access.0.reusable_until
        {
            entry.merge_plans = auth.merge_plans.clone();
            return Ok(entry.access.clone());
        }
        *cached = None;
        let access = AccessToken(Arc::new(
            self.issue_access_token(target, auth, audience).await?,
        ));
        *cached = Some(CachedHostedAccess {
            identity,
            access: access.clone(),
            merge_plans: auth.merge_plans.clone(),
        });
        Ok(access)
    }

    pub(super) async fn invalidate_access(&self, access: &AccessToken) {
        let mut cached = self.hosted_access.lock().await;
        if cached
            .as_ref()
            .is_some_and(|entry| Arc::ptr_eq(&entry.access.0, &access.0))
        {
            *cached = None;
        }
    }

    pub(super) async fn invalidate_rejected_access(
        &self,
        response: &Response,
        access: Option<&AccessToken>,
    ) {
        if rejects_access(response.status(), response.headers())
            && let Some(access) = access
        {
            self.invalidate_access(access).await;
        }
    }
}

fn rejects_access(status: StatusCode, headers: &HeaderMap) -> bool {
    status == StatusCode::UNAUTHORIZED
        || (status == StatusCode::FORBIDDEN
            && headers
                .get_all(WWW_AUTHENTICATE)
                .iter()
                .any(is_invalid_bearer_token))
}

fn is_invalid_bearer_token(challenge: &HeaderValue) -> bool {
    let Ok(challenge) = challenge.to_str() else {
        return false;
    };
    let challenge = challenge.trim();
    let Some(separator) = challenge.find(|character: char| character.is_ascii_whitespace()) else {
        return false;
    };
    let (scheme, parameters) = challenge.split_at(separator);
    scheme.eq_ignore_ascii_case("bearer")
        && parameters.split(',').any(|parameter| {
            let Some((name, value)) = parameter.trim().split_once('=') else {
                return false;
            };
            name.trim().eq_ignore_ascii_case("error")
                && matches!(value.trim(), "invalid_token" | r#""invalid_token""#)
        })
}

#[cfg(test)]
#[path = "access/tests.rs"]
mod tests;
