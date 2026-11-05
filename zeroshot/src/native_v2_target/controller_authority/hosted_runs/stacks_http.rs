use openengine_cluster_protocol::{Stack, StackId, StackSubmitRequest};
use reqwest::header::ACCEPT;

use super::{
    CACHE_CONTROL, StacksDescriptor, TargetAccess, TargetAuthorityError,
    TargetHttpControlAuthority, TargetRecord, authority_error,
};
use super::super::access::AccessToken;

const MAX_STACK_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy)]
enum StackHttpOperation<'a> {
    Submit(&'a StackSubmitRequest),
    Status(&'a StackId),
    Force(&'a StackId),
}

impl TargetHttpControlAuthority {
    async fn require_stack_access(
        &self,
        target: &TargetRecord,
    ) -> Result<(StacksDescriptor, AccessToken), TargetAuthorityError> {
        if matches!(target.access, TargetAccess::Direct) {
            return Err(authority_error(
                "direct target does not support hosted stacks",
            ));
        }
        if let Some(access) = self.hosted_access.lock().await.as_ref()
            && let Some(access) = access.stack_access(target)
        {
            return Ok(access);
        }
        let (auth, controller) = self.descriptors(target).await?;
        let routes = auth.stacks.clone().ok_or_else(|| {
            authority_error("hosted target does not advertise zeroshot.stacks/v1")
        })?;
        let access = self
            .access_token(target, &auth, &controller.audience)
            .await?;
        Ok((routes, access))
    }

    async fn execute_stack_operation(
        &self,
        routes: &StacksDescriptor,
        access: &AccessToken,
        operation: StackHttpOperation<'_>,
    ) -> Result<Stack, TargetAuthorityError> {
        let (request, label) = match operation {
            StackHttpOperation::Submit(body) => {
                let url = routes.create_url()?;
                let request = self
                    .authorized(self.client.post(url), access)?
                    .header(CACHE_CONTROL, "no-store")
                    .json(body);
                (request, "stack submission")
            }
            StackHttpOperation::Status(stack_id) => {
                let url = routes.status_url(stack_id)?;
                let request = self
                    .authorized(self.client.get(url), access)?
                    .header(ACCEPT, "application/json")
                    .header(CACHE_CONTROL, "no-store");
                (request, "stack status")
            }
            StackHttpOperation::Force(stack_id) => {
                let url = routes.force_url(stack_id)?;
                let request = self
                    .authorized(self.client.post(url), access)?
                    .header(CACHE_CONTROL, "no-store")
                    .json(&serde_json::json!({}));
                (request, "stack force-stop")
            }
        };
        self.hosted_json((request, access), label, Some(MAX_STACK_RESPONSE_BYTES))
            .await
    }

    async fn execute_stack_with_access(
        &self,
        target: &TargetRecord,
        operation: StackHttpOperation<'_>,
    ) -> Result<Stack, TargetAuthorityError> {
        let mut retried = false;
        loop {
            let (routes, access) = self.require_stack_access(target).await?;
            match self
                .execute_stack_operation(&routes, &access, operation)
                .await
            {
                Err(error) if error.is_http_auth_rejection() && self.machine.is_none() => {
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

    pub(in super::super) async fn stack_submit(
        &self,
        target: &TargetRecord,
        body: &StackSubmitRequest,
    ) -> Result<Stack, TargetAuthorityError> {
        self.execute_stack_with_access(target, StackHttpOperation::Submit(body))
            .await
    }

    pub(in super::super) async fn stack_status(
        &self,
        target: &TargetRecord,
        stack_id: &StackId,
    ) -> Result<Stack, TargetAuthorityError> {
        self.execute_stack_with_access(target, StackHttpOperation::Status(stack_id))
            .await
    }

    pub(in super::super) async fn stack_force(
        &self,
        target: &TargetRecord,
        stack_id: &StackId,
    ) -> Result<Stack, TargetAuthorityError> {
        self.execute_stack_with_access(target, StackHttpOperation::Force(stack_id))
            .await
    }
}
