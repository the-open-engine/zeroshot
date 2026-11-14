#[path = "review/identity.rs"]
mod identity;

use reqwest::header::{ACCEPT, CACHE_CONTROL};
use zeroshot_engine::native_v2_review::{PreparedReview, ReviewResolutionRequest};

use super::{RequestCapabilities, TargetHttpControlAuthority};
use super::contract::{authority_error, read_success_json_with_limit};
use crate::native_v2_target::{TargetAccess, TargetAuthorityError, TargetRecord, TargetSessionPurpose};

/// Direct targets resolve reviews with the client's token, which the CLI sends only on opt-in.
const DIRECT_REVIEW_NEEDS_GH_TOKEN: &str =
    "direct task resolution needs GH_TOKEN; pass --forward-gh-token to send it";

impl TargetHttpControlAuthority {
    pub(super) async fn resolve_review_request(
        &self,
        target: &TargetRecord,
        request: ReviewResolutionRequest,
        resolution: zeroshot_engine::native_v2_review::ReviewResolutionAccess<'_>,
    ) -> Result<PreparedReview, TargetAuthorityError> {
        let (controller, access) = self
            .controller_access_inner(
                target,
                TargetSessionPurpose::General,
                RequestCapabilities {
                    runtime: Some(resolution.runtime),
                    task_resolution: true,
                    request_ask: false,
                    request_wait: false,
                    ..Default::default()
                },
            )
            .await?;
        let url = controller
            .task_resolution
            .ok_or_else(|| authority_error("target does not advertise task resolution"))?;
        let body = if matches!(target.access, TargetAccess::Direct) {
            // The CLI decides whether the shell token may leave it; never read it here.
            let token = resolution
                .github_token
                .ok_or_else(|| authority_error(DIRECT_REVIEW_NEEDS_GH_TOKEN))?;
            super::contract::validate_secret(token, "GitHub credential")?;
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
