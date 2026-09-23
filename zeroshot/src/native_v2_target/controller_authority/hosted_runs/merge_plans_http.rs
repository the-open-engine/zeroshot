use openengine_cluster_protocol::{MergePlan, MergePlanId, MergePlanSubmitRequest};
use reqwest::header::ACCEPT;

use super::{
    CACHE_CONTROL, MergePlansDescriptor, TargetAccess, TargetAuthorityError,
    TargetHttpControlAuthority, TargetRecord, authority_error,
};
use super::super::access::AccessToken;

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
    ) -> Result<(MergePlansDescriptor, AccessToken), TargetAuthorityError> {
        if matches!(target.access, TargetAccess::Direct) {
            return Err(authority_error(
                "direct target does not support hosted merge plans",
            ));
        }
        if let Some(access) = self.hosted_access.lock().await.as_ref()
            && let Some(access) = access.merge_plan_access(target)
        {
            return Ok(access);
        }
        let (auth, controller) = self.descriptors(target).await?;
        let routes = auth.merge_plans.clone().ok_or_else(|| {
            authority_error("hosted target does not advertise zeroshot.merge-plans/v1")
        })?;
        let access = self
            .access_token(target, &auth, &controller.audience)
            .await?;
        Ok((routes, access))
    }

    async fn execute_merge_plan_operation(
        &self,
        routes: &MergePlansDescriptor,
        access: &AccessToken,
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
        self.hosted_json(
            (request, access),
            label,
            Some(MAX_MERGE_PLAN_RESPONSE_BYTES),
        )
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
                    self.invalidate_access(&access).await;
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
