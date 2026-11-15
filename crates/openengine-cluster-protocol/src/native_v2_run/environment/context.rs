use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::super::{GitRemoteUrl, NativeV2RunValueError, ResolvedSource, SourceBranchId};

/// A credential-free locator in the same forms as the CLI's `--repository`.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ContextRepository {
    #[schemars(length(min = 1, max = 2048))]
    pub repository: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<SourceBranchId>,
}

impl ContextRepository {
    pub fn remote_url(&self) -> Result<GitRemoteUrl, NativeV2RunValueError> {
        if let Ok(remote) = GitRemoteUrl::new(&self.repository) {
            return Ok(remote);
        }
        super::super::validate_legacy_repository(&self.repository).map_err(|_| {
            NativeV2RunValueError(
                "context repository must be GitHub owner/name or credential-free HTTPS/SSH",
            )
        })?;
        if self
            .repository
            .split('/')
            .any(|part| matches!(part, "." | ".."))
        {
            return Err(NativeV2RunValueError(
                "context repository must name an owner and repository",
            ));
        }
        GitRemoteUrl::new(format!("https://github.com/{}.git", self.repository))
    }

    /// Safe relative checkout path; also excludes Git's administrative directory names.
    pub fn repository_path(&self) -> Result<String, NativeV2RunValueError> {
        let (_, path) = locator_parts(self.remote_url()?.as_str())?;
        Ok(path)
    }

    /// Check authority before a client sends source credentials to the context remote.
    pub fn validate_source(&self, source: &ResolvedSource) -> Result<(), NativeV2RunValueError> {
        let expected = locator_parts(self.remote_url()?.as_str())?;
        let primary = source_locator(source)?;
        if expected.0 != primary.0 {
            return Err(NativeV2RunValueError(
                "context repository must use the source repository's forge host",
            ));
        }
        if expected.1.eq_ignore_ascii_case(&primary.1) {
            return Err(NativeV2RunValueError(
                "context repository names the run's own source repository",
            ));
        }
        Ok(())
    }
}

fn locator_parts(remote: &str) -> Result<(String, String), NativeV2RunValueError> {
    let invalid = NativeV2RunValueError("context repository URL must have a safe repository path");
    // Compare only literal paths. Forges decode escapes, and URL parsing can normalize escaped
    // dot segments before we inspect the path, hiding duplicate or primary-source aliases.
    if remote.contains('%') {
        return Err(NativeV2RunValueError(
            "context repository URL must not contain percent escapes",
        ));
    }
    let url = if remote.contains("://") {
        url::Url::parse(remote)
    } else {
        let delimiter = if let Some(start) = remote.find('[') {
            remote[start..].find("]:").map(|end| start + end + 1)
        } else {
            remote.find(':')
        }
        .ok_or(invalid.clone())?;
        let (authority, path) = remote.split_at(delimiter);
        url::Url::parse(&format!("ssh://{authority}/{}", &path[1..]))
    }
    .map_err(|_| invalid.clone())?;
    let host = url.host_str().ok_or(invalid.clone())?.to_ascii_lowercase();
    let host = match url.port() {
        Some(22) if url.scheme() == "ssh" => host,
        Some(port) => format!("{host}:{port}"),
        None => host,
    };
    let path = url.path().trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if path.is_empty()
        || path.len() > 255
        || path.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.eq_ignore_ascii_case(".git")
        })
    {
        return Err(invalid);
    }
    Ok((host, path.to_owned()))
}

fn source_locator(source: &ResolvedSource) -> Result<(String, String), NativeV2RunValueError> {
    match &source.remote {
        Some(remote) => locator_parts(remote.url.as_str()),
        None => locator_parts(&format!(
            "https://github.com/{}.git",
            source.repository.as_str()
        )),
    }
}

pub(super) fn validate_repositories(
    repositories: &[ContextRepository],
) -> Result<(), NativeV2RunValueError> {
    if repositories.len() > 16 {
        return Err(NativeV2RunValueError(
            "contextRepositories exceeds the 16 repository limit",
        ));
    }
    let mut paths = BTreeSet::new();
    for repository in repositories {
        if repository.branch.as_ref().is_some_and(|branch| {
            let branch = branch.as_str();
            branch == "HEAD"
                || branch
                    .split('/')
                    .any(|part| part.is_empty() || part.starts_with('.') || part.ends_with(".lock"))
        }) {
            return Err(NativeV2RunValueError(
                "context repository branch is not a valid Git branch",
            ));
        }
        let path = repository.repository_path()?.to_ascii_lowercase();
        if !paths.insert(path) {
            return Err(NativeV2RunValueError(
                "contextRepositories contains a duplicate repository path",
            ));
        }
    }
    // A checkout cannot be nested inside another checkout.
    if paths.iter().any(|path| {
        paths
            .iter()
            .any(|other| path.starts_with(&format!("{other}/")))
    }) {
        return Err(NativeV2RunValueError(
            "contextRepositories contains overlapping repository paths",
        ));
    }
    Ok(())
}

/// Admission checks the complete wish list against immutable, ordered client pins.
pub fn validate_context_sources(
    environment: Option<&super::RuntimeEnvironment>,
    sources: &[ResolvedSource],
    source: Option<&ResolvedSource>,
) -> Result<(), NativeV2RunValueError> {
    let repositories = environment.map_or(&[][..], |environment| {
        environment.context_repositories.as_slice()
    });
    validate_repositories(repositories)?;
    if repositories.len() != sources.len() {
        return Err(NativeV2RunValueError(
            "contextSources must match contextRepositories one to one",
        ));
    }
    if sources.is_empty() {
        return Ok(());
    }
    let source = source.ok_or(NativeV2RunValueError(
        "context repositories require a source repository",
    ))?;
    for (repository, pin) in repositories.iter().zip(sources) {
        repository.validate_source(source)?;
        let expected = locator_parts(repository.remote_url()?.as_str())?;
        if source_locator(pin)? != expected
            || pin.repository.as_str() != expected.1
            || repository
                .branch
                .as_ref()
                .is_some_and(|branch| branch != &pin.branch)
            || pin
                .remote
                .as_ref()
                .and_then(|remote| remote.connection.as_ref())
                != source
                    .remote
                    .as_ref()
                    .and_then(|remote| remote.connection.as_ref())
        {
            return Err(NativeV2RunValueError(
                "contextSources repository, branch or credential authority does not match contextRepositories",
            ));
        }
    }
    Ok(())
}
