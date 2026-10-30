//! GitHub instance routing and the explicit supported Enterprise Server baseline.
use super::*;
use openengine_cluster_protocol::SourceForgeBaseUrl;

impl GhCliDeliveryAuthority {
    pub fn with_base_url(mut self, base: SourceForgeBaseUrl) -> Result<Self, SourceDeliveryError> {
        let url = url::Url::parse(base.as_str()).map_err(|_| SourceDeliveryError::Rejected)?;
        if url.path() != "/" {
            return Err(SourceDeliveryError::identity(
                "GitHub requires a root instance base URL",
            ));
        }
        self.base_url = base;
        self.instance_verified = Arc::new(tokio::sync::OnceCell::new());
        Ok(self)
    }

    pub(super) fn hostname(&self) -> String {
        let url = url::Url::parse(self.base_url.as_str()).expect("validated instance URL");
        url[url::Position::BeforeHost..url::Position::AfterPort].to_owned()
    }

    pub(super) fn is_cloud(&self) -> bool {
        let host = self.hostname();
        host == "github.com" || host.ends_with(".ghe.com")
    }

    pub(super) fn api_command(&self, credential: SourceDeliveryCredential<'_>) -> Command {
        let mut command = clean_command(&self.config, &self.config.gh_program, credential);
        command
            .env("GH_HOST", self.hostname())
            .env_remove("GH_TOKEN")
            .env_remove("GH_ENTERPRISE_TOKEN");
        let field = if self.is_cloud() {
            "GH_TOKEN"
        } else {
            "GH_ENTERPRISE_TOKEN"
        };
        command.env(field, credential.expose());
        command
    }

    pub(super) async fn ensure_instance(
        &self,
        credential: SourceDeliveryCredential<'_>,
    ) -> Result<(), SourceDeliveryError> {
        if self.is_cloud() {
            return Ok(());
        }
        self.instance_verified.get_or_try_init(|| async {
            let output = self.raw_api_output(&["meta".to_owned()], credential).await?;
            let value: serde_json::Value = serde_json::from_slice(&output)
                .map_err(|_| SourceDeliveryError::identity("GitHub Enterprise Server metadata was invalid"))?;
            let version = value.get("installed_version").and_then(serde_json::Value::as_str)
                .and_then(server_version)
                .ok_or_else(|| SourceDeliveryError::identity("GitHub Enterprise Server version is unavailable; 3.22.2 or newer is required"))?;
            if version < (3, 22, 2) {
                return Err(SourceDeliveryError::identity("GitHub Enterprise Server 3.22.2 or newer is required"));
            }
            Ok(())
        }).await.map(|_| ())
    }

    pub(super) fn canonical_review_links(
        &self,
        review: &SourceReviewReceipt,
    ) -> Result<SourceReviewLinks, SourceDeliveryError> {
        if !super::super::valid_github_repository(&review.repository)
            || review
                .review_id
                .parse::<u64>()
                .ok()
                .filter(|id| *id > 0)
                .map(|id| id.to_string())
                .as_deref()
                != Some(&review.review_id)
            || !valid_revision(&review.head_revision)
        {
            return Err(SourceDeliveryError::Rejected);
        }
        let repository = format!("{}/{}", self.base_url, review.repository);
        Ok(SourceReviewLinks {
            url: format!("{repository}/pull/{}", review.review_id),
            commit_url: format!("{repository}/commit/{}", review.head_revision),
        })
    }
}

fn server_version(value: &str) -> Option<(u64, u64, u64)> {
    let parts = value
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    match parts.as_slice() {
        [major, minor, patch] => Some((*major, *minor, *patch)),
        _ => None,
    }
}

#[cfg(all(test, unix))]
#[path = "instance/tests.rs"]
mod tests;
