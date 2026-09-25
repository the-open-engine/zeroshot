use openengine_cluster_protocol::{
    RunProfile, RunProfileDefaultRequest, RunProfileDefaultResult, RunProfileDeleteResult,
    RunProfileListRequest, RunProfileListResult, RunProfileMutationResult, RunProfileRunRequest,
    RunProfileSelector, RunProfileSetRequest, RunSubmitResult,
};
use reqwest::header::{ACCEPT, CACHE_CONTROL};

use super::contract::{RunProfilesDescriptor, authority_error};
use super::TargetHttpControlAuthority;
use super::access::AccessToken;
use crate::native_v2_target::{TargetAccess, TargetAuthorityError, TargetRecord};

// Includes worst-case JSON escaping of both scripts and the bounded public-variable map.
const MAX_ENVIRONMENT_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

enum ProfileOperation {
    List,
    Show,
    Set,
    Delete,
    Default,
    Run,
}

impl ProfileOperation {
    fn route<'a>(&self, routes: &'a RunProfilesDescriptor) -> &'a reqwest::Url {
        match self {
            Self::List => &routes.list,
            Self::Show => &routes.show,
            Self::Set => &routes.set,
            Self::Delete => &routes.delete,
            Self::Default => &routes.default,
            Self::Run => &routes.run,
        }
    }

    const fn label(&self) -> &'static str {
        match self {
            Self::List => "profile list",
            Self::Show => "profile show",
            Self::Set => "profile set",
            Self::Delete => "profile delete",
            Self::Default => "profile default",
            Self::Run => "profile run",
        }
    }
}

impl TargetHttpControlAuthority {
    pub(super) async fn environment_show(
        &self,
        target: &TargetRecord,
        scope: openengine_cluster_protocol::RunProfileScope,
        id: openengine_cluster_protocol::EnvironmentId,
    ) -> Result<openengine_cluster_protocol::RuntimeEnvironmentResource, TargetAuthorityError> {
        if matches!(target.access, TargetAccess::Direct) {
            return Err(authority_error(
                "direct targets accept resolved environments",
            ));
        }
        let (auth, controller) = self.descriptors(target).await?;
        let routes = auth
            .runtime_environments
            .as_ref()
            .ok_or_else(|| authority_error("target does not advertise environment resources"))?;
        let url = routes.show(scope, &id)?;
        let access = self
            .access_token(target, &auth, &controller.audience)
            .await?;
        let builder = self
            .authorized(self.client.get(url), &access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store");
        let resource: openengine_cluster_protocol::RuntimeEnvironmentResource = self
            .hosted_json(
                (builder, &access),
                "environment show",
                Some(MAX_ENVIRONMENT_RESPONSE_BYTES),
            )
            .await?;
        if resource.id != id {
            return Err(authority_error("target returned another environment"));
        }
        resource
            .definition
            .validate()
            .map_err(|error| authority_error(error.to_string()))?;
        Ok(resource)
    }

    async fn profile_access(
        &self,
        target: &TargetRecord,
    ) -> Result<(RunProfilesDescriptor, AccessToken), TargetAuthorityError> {
        if matches!(target.access, TargetAccess::Direct) {
            return Err(authority_error(
                "direct target does not advertise profile management",
            ));
        }
        let (auth, controller) = self.descriptors(target).await?;
        let routes = auth.run_profiles.clone().ok_or_else(|| {
            authority_error("hosted target does not advertise profile management")
        })?;
        let access = self
            .access_token(target, &auth, &controller.audience)
            .await?;
        Ok((routes, access))
    }

    async fn profile_json<I, O>(
        &self,
        target: &TargetRecord,
        operation: ProfileOperation,
        input: &I,
    ) -> Result<O, TargetAuthorityError>
    where
        I: serde::Serialize + Sync,
        O: serde::de::DeserializeOwned,
    {
        let (routes, access) = self.profile_access(target).await?;
        let url = operation.route(&routes).clone();
        let builder = self
            .authorized(self.client.post(url.clone()), &access)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .json(input);
        self.hosted_json((builder, &access), operation.label(), None)
            .await
    }

    pub(super) async fn profile_list(
        &self,
        target: &TargetRecord,
        request: RunProfileListRequest,
    ) -> Result<RunProfileListResult, TargetAuthorityError> {
        self.profile_json(target, ProfileOperation::List, &request)
            .await
    }

    pub(super) async fn profile_show(
        &self,
        target: &TargetRecord,
        selector: RunProfileSelector,
    ) -> Result<RunProfile, TargetAuthorityError> {
        self.profile_json(target, ProfileOperation::Show, &selector)
            .await
    }

    pub(super) async fn profile_set(
        &self,
        target: &TargetRecord,
        request: RunProfileSetRequest,
    ) -> Result<RunProfileMutationResult, TargetAuthorityError> {
        self.profile_json(target, ProfileOperation::Set, &request)
            .await
    }

    pub(super) async fn profile_delete(
        &self,
        target: &TargetRecord,
        selector: RunProfileSelector,
    ) -> Result<RunProfileDeleteResult, TargetAuthorityError> {
        self.profile_json(target, ProfileOperation::Delete, &selector)
            .await
    }

    pub(super) async fn profile_default(
        &self,
        target: &TargetRecord,
        request: RunProfileDefaultRequest,
    ) -> Result<RunProfileDefaultResult, TargetAuthorityError> {
        self.profile_json(target, ProfileOperation::Default, &request)
            .await
    }

    pub(super) async fn profile_run(
        &self,
        target: &TargetRecord,
        request: &RunProfileRunRequest,
    ) -> Result<RunSubmitResult, TargetAuthorityError> {
        self.profile_json(target, ProfileOperation::Run, request)
            .await
    }
}

#[cfg(test)]
#[path = "profiles/tests.rs"]
mod tests;
