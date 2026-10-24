#[path = "review/identity.rs"]
mod identity;

use reqwest::header::{ACCEPT, CACHE_CONTROL};
use zeroshot_engine::native_v2_review::{PreparedReview, ReviewResolutionRequest};

use super::{RequestCapabilities, TargetHttpControlAuthority};
use super::contract::{authority_error, read_success_json_with_limit};
use crate::native_v2_target::{TargetAccess, TargetAuthorityError, TargetRecord, TargetSessionPurpose};

impl TargetHttpControlAuthority {
    pub(super) async fn resolve_review_request(
        &self,
        target: &TargetRecord,
        request: ReviewResolutionRequest,
        runtime: &openengine_cluster_protocol::RuntimePlan,
    ) -> Result<PreparedReview, TargetAuthorityError> {
        let (controller, access) = self
            .controller_access_inner(
                target,
                TargetSessionPurpose::General,
                RequestCapabilities {
                    runtime: Some(runtime),
                    task_resolution: true,
                    ..Default::default()
                },
            )
            .await?;
        let url = controller
            .task_resolution
            .ok_or_else(|| authority_error("target does not advertise task resolution"))?;
        let body = if matches!(target.access, TargetAccess::Direct) {
            let token = std::env::var("GH_TOKEN")
                .or_else(|_| std::env::var("GITHUB_TOKEN"))
                .map_err(|_| authority_error("direct task resolution requires GH_TOKEN"))?;
            super::contract::validate_secret(&token, "GitHub credential")?;
            serde_json::json!({"request":request,"githubToken":token})
        } else {
            serde_json::to_value(request)
                .map_err(|_| authority_error("review resolution input is invalid"))?
        };
        let builder = self
            .with_access(self.client.post(url.clone()), access.as_deref())?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .json(&body);
        if let Some(access) = access.as_ref() {
            return self
                .hosted_json(
                    (builder, access),
                    "review resolution",
                    Some(2 * 1024 * 1024),
                )
                .await;
        }
        let response = builder
            .send()
            .await
            .map_err(|error| TargetAuthorityError::request_failed("review resolution", &error))?;
        read_success_json_with_limit(response, &url, "review resolution", 2 * 1024 * 1024).await
    }
}
