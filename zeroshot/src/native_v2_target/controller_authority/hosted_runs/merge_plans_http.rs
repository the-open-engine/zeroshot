use std::time::Instant;

use openengine_cluster_protocol::{MergePlan, MergePlanId, MergePlanSubmitRequest};
use reqwest::header::ACCEPT;

use super::{
    CACHE_CONTROL, MergePlansDescriptor, TargetAccess, TargetAuthorityError,
    TargetHttpControlAuthority, TargetRecord, authority_error,
};
use super::super::CachedMergePlanAccess;

const MAX_MERGE_PLAN_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy)]
enum MergePlanHttpOperation<'a> {
    Submit(&'a MergePlanSubmitRequest),
    Status(&'a MergePlanId),
    Force(&'a MergePlanId),
}

impl TargetHttpControlAuthority {
    async fn require_merge_plan_access(
        &self,
        target: &TargetRecord,
    ) -> Result<(MergePlansDescriptor, String), TargetAuthorityError> {
        if matches!(target.access, TargetAccess::Direct) {
            return Err(authority_error(
                "direct target does not support hosted merge plans",
            ));
        }
        let mut cached = self.merge_plan_access.lock().await;
        if let Some(access) = cached.as_ref()
            && access.target == *target
            && Instant::now() < access.reusable_until
        {
            return Ok((access.routes.clone(), access.access_token.clone()));
        }
        let (auth, controller) = self.descriptors(target).await?;
        let routes = auth.merge_plans.clone().ok_or_else(|| {
            authority_error("hosted target does not advertise zeroshot.merge-plans/v1")
        })?;
        let access = self
            .issue_access_token(target, &auth, &controller.audience)
            .await?;
        *cached = Some(CachedMergePlanAccess {
            target: target.clone(),
            routes: routes.clone(),
            access_token: access.value.clone(),
            reusable_until: access.reusable_until,
        });
        Ok((routes, access.value))
    }

    async fn invalidate_merge_plan_access(&self, target: &TargetRecord, access_token: &str) {
        let mut cached = self.merge_plan_access.lock().await;
        if cached
            .as_ref()
            .is_some_and(|access| access.target == *target && access.access_token == access_token)
        {
            *cached = None;
        }
    }

    async fn execute_merge_plan_operation(
        &self,
        routes: &MergePlansDescriptor,
        access: &str,
        operation: MergePlanHttpOperation<'_>,
    ) -> Result<MergePlan, TargetAuthorityError> {
        let (request, label) = match operation {
            MergePlanHttpOperation::Submit(body) => {
                let url = routes.create_url()?;
                let request = self
                    .authorized(self.client.post(url), access)?
                    .header(CACHE_CONTROL, "no-store")
                    .json(body);
                (request, "merge-plan submission")
            }
            MergePlanHttpOperation::Status(plan_id) => {
                let url = routes.status_url(plan_id)?;
                let request = self
                    .authorized(self.client.get(url), access)?
                    .header(ACCEPT, "application/json")
                    .header(CACHE_CONTROL, "no-store");
                (request, "merge-plan status")
            }
            MergePlanHttpOperation::Force(plan_id) => {
                let url = routes.force_url(plan_id)?;
                let request = self
                    .authorized(self.client.post(url), access)?
                    .header(CACHE_CONTROL, "no-store")
                    .json(&serde_json::json!({}));
                (request, "merge-plan force-stop")
            }
        };
        self.hosted_json(request, label, Some(MAX_MERGE_PLAN_RESPONSE_BYTES))
            .await
    }

    async fn execute_merge_plan_with_access(
        &self,
        target: &TargetRecord,
        operation: MergePlanHttpOperation<'_>,
    ) -> Result<MergePlan, TargetAuthorityError> {
        let mut retried = false;
        loop {
            let (routes, access) = self.require_merge_plan_access(target).await?;
            match self
                .execute_merge_plan_operation(&routes, &access, operation)
                .await
            {
                Err(error) if error.is_http_auth_rejection() => {
                    self.invalidate_merge_plan_access(target, &access).await;
                    if retried {
                        return Err(error);
                    }
                    retried = true;
                }
                result => return result,
            }
        }
    }

    pub(in super::super) async fn merge_plan_submit(
        &self,
        target: &TargetRecord,
        body: &MergePlanSubmitRequest,
    ) -> Result<MergePlan, TargetAuthorityError> {
        self.execute_merge_plan_with_access(target, MergePlanHttpOperation::Submit(body))
            .await
    }

    pub(in super::super) async fn merge_plan_status(
        &self,
        target: &TargetRecord,
        plan_id: &MergePlanId,
    ) -> Result<MergePlan, TargetAuthorityError> {
        self.execute_merge_plan_with_access(target, MergePlanHttpOperation::Status(plan_id))
            .await
    }

    pub(in super::super) async fn merge_plan_force(
        &self,
        target: &TargetRecord,
        plan_id: &MergePlanId,
    ) -> Result<MergePlan, TargetAuthorityError> {
        self.execute_merge_plan_with_access(target, MergePlanHttpOperation::Force(plan_id))
            .await
    }
}
