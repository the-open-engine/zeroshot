//! Secret-free instance binding and common locator checks. Provider grammar stays in adapters.
use openengine_cluster_protocol::{GitRemoteUrl, SourceForgeBaseUrl, SourceForgeIdentity};
use crate::source_code_provider::{SourceCommentDestination, SourceDeliveryError};
use super::SourceDeliveryFactoryError;

pub fn forge_base_url(
    forge: &SourceForgeIdentity,
) -> Result<SourceForgeBaseUrl, SourceDeliveryFactoryError> {
    if let Some(base) = &forge.base_url {
        return Ok(base.clone());
    }
    let default = match forge.provider.as_str() {
        super::GITHUB_PROVIDER_ID => "https://github.com",
        super::GITLAB_PROVIDER_ID => "https://gitlab.com",
        _ => {
            return Err(SourceDeliveryFactoryError::Instance(
                "source provider requires an instance base URL",
            ));
        }
    };
    SourceForgeBaseUrl::new(default).map_err(|_| SourceDeliveryFactoryError::RemoteIdentity)
}

/// Recognizes a locator only inside an explicitly selected instance.
pub(super) fn repository_at_remote(remote: &str, base: &SourceForgeBaseUrl) -> Option<String> {
    let parsed = remote_url(remote)?;
    let base = url::Url::parse(base.as_str()).ok()?;
    if !parsed.host_str()?.eq_ignore_ascii_case(base.host_str()?) {
        return None;
    }
    let path = repository_path(&parsed, &base)?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    valid_repository_path(path).then(|| path.to_owned())
}

fn remote_url(value: &str) -> Option<url::Url> {
    let remote = GitRemoteUrl::new(value).ok()?;
    if remote.as_str().contains("://") {
        return url::Url::parse(remote.as_str()).ok();
    }
    let (authority, path) = remote.as_str().split_once(':')?;
    url::Url::parse(&format!("ssh://{authority}/{path}")).ok()
}

fn repository_path<'a>(remote: &'a url::Url, base: &url::Url) -> Option<&'a str> {
    match remote.scheme() {
        "https" if remote.origin() == base.origin() => strip_base_path(remote.path(), base.path()),
        "ssh" => Some(remote.path().trim_start_matches('/')),
        _ => None,
    }
}

fn valid_repository_path(path: &str) -> bool {
    !path.is_empty()
        && path
            .split('/')
            .all(|p| !p.is_empty() && !matches!(p, "." | ".."))
}

pub(super) fn strip_base_path<'a>(path: &'a str, base: &str) -> Option<&'a str> {
    let base = base.trim_end_matches('/');
    path.strip_prefix(base)?.strip_prefix('/')
}

pub(super) fn public_discussion_url(value: &str) -> Option<url::Url> {
    if value.len() > 2048 || value.chars().any(char::is_control) {
        return None;
    }
    let parsed = url::Url::parse(value).ok()?;
    (parsed.scheme() == "https"
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.as_str() == value)
        .then_some(parsed)
}

/// Common captured comparison fields, independent of a forge's repository grammar.
pub fn validate_comment_review_scope(
    destination: &SourceCommentDestination,
    scope: &serde_json::Value,
) -> Result<(), SourceDeliveryError> {
    use crate::source_code_provider::SourceDiscussionKind;
    let fields = scope.as_object().ok_or(SourceDeliveryError::Rejected)?;
    if fields.is_empty() {
        return (destination.kind == SourceDiscussionKind::Issue)
            .then_some(())
            .ok_or(SourceDeliveryError::Rejected);
    }
    let names = [
        "baseRepository",
        "baseBranch",
        "baseRevision",
        "headRepository",
        "headBranch",
        "headRevision",
    ];
    if destination.kind != SourceDiscussionKind::PullRequest || fields.len() != names.len() {
        return Err(SourceDeliveryError::Rejected);
    }
    for name in names {
        let value = fields
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or(SourceDeliveryError::Rejected)?;
        if !valid_scope_field(name, value) {
            return Err(SourceDeliveryError::Rejected);
        }
    }
    if fields["baseRepository"].as_str() != Some(&destination.repository) {
        return Err(SourceDeliveryError::Rejected);
    }
    Ok(())
}

fn valid_scope_field(name: &str, value: &str) -> bool {
    if name.ends_with("Revision") {
        crate::source_code_provider::valid_git_revision(value)
    } else if name.ends_with("Branch") {
        crate::source_code_provider::valid_git_branch(value)
    } else {
        crate::source_code_provider::valid_delivery_repository(value)
    }
}
