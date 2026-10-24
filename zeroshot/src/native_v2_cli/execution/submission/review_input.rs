//! Reuse one resolved review snapshot across uncertain submission retries.
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::execution::platform::{FileAccess, private_directory, private_file};
use crate::native_v2_cli::{NativeV2CliBackend, NativeV2CliError, RunCommand, RunInput};
use crate::native_v2_review::{PreparedReview, ReviewResolutionRequest};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SavedReview {
    identity: SnapshotIdentity,
    prepared: PreparedReview,
}

struct Guard(File);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub(super) async fn resolve<B: NativeV2CliBackend>(
    run: &RunCommand,
    profile: &super::ResolvedRunProfile,
    backend: &B,
) -> Result<super::SubmissionInput, NativeV2CliError> {
    let mut input = super::SubmissionInput::capture(run)?;
    let Some(request) = review_request(run) else {
        return Ok(input);
    };
    input.review = Some(
        if let Some(cache) = cache_for(run, profile, backend, input.environment.as_ref()).await? {
            cache.prepare(run, &profile.runtime, backend).await?
        } else {
            fresh_review(run, backend, request, &profile.runtime).await?
        },
    );
    Ok(input)
}

fn review_request(run: &RunCommand) -> Option<ReviewResolutionRequest> {
    let RunInput::TaskFrom(url) = &run.input else {
        return None;
    };
    Some(ReviewResolutionRequest {
        url: url.clone(),
        branch: run.branch.clone(),
        revision: run.revision.clone(),
    })
}

async fn cache_for<B: NativeV2CliBackend>(
    run: &RunCommand,
    profile: &super::ResolvedRunProfile,
    backend: &B,
    environment: Option<&openengine_cluster_protocol::RuntimeEnvironment>,
) -> Result<Option<ReviewCache>, NativeV2CliError> {
    let Some(key) = &run.submission_key else {
        return Ok(None);
    };
    let Some(namespace) = backend
        .review_cache_namespace(run.target.as_deref())
        .await?
    else {
        return Ok(None);
    };
    let identity = snapshot_identity(run, profile, environment)?;
    if let Some(workspace) = &identity.workspace {
        crate::native_v2_local::validate_workspace_state(
            workspace,
            &crate::native_v2_cli::default_local_state_root()?,
        )
        .map_err(|error| invalid(&error.to_string()))?;
    }
    ReviewCache::open(&namespace, key, identity).await.map(Some)
}

async fn fresh_review<B: NativeV2CliBackend>(
    run: &RunCommand,
    backend: &B,
    request: ReviewResolutionRequest,
    runtime: &openengine_cluster_protocol::RuntimePlan,
) -> Result<PreparedReview, NativeV2CliError> {
    let mut prepared = backend
        .resolve_review(run.target.as_deref(), request, runtime)
        .await?;
    local_snapshot(run, &mut prepared, false).await?;
    Ok(prepared)
}

#[derive(Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SnapshotIdentity {
    request: ReviewResolutionRequest,
    workspace: Option<PathBuf>,
    authored_digest: String,
}

fn snapshot_identity(
    run: &RunCommand,
    profile: &super::ResolvedRunProfile,
    environment: Option<&openengine_cluster_protocol::RuntimeEnvironment>,
) -> Result<SnapshotIdentity, NativeV2CliError> {
    let workspace = run
        .target
        .is_none()
        .then(|| {
            crate::native_v2_local::local_resolved_source(
                run.cwd.as_deref().unwrap_or(std::path::Path::new(".")),
                std::path::Path::new("git"),
            )
            .map(|(workspace, _)| workspace)
            .map_err(|error| invalid(&error.to_string()))
        })
        .transpose()?;
    let authored = serde_json::to_vec(&(
        &run.title,
        &profile.graph,
        &profile.runtime,
        &profile.remote_selector,
        environment,
    ))
    .map_err(|_| invalid("review retry identity is invalid"))?;
    Ok(SnapshotIdentity {
        request: review_request(run).ok_or_else(|| invalid("review input is unavailable"))?,
        workspace,
        authored_digest: format!("{:x}", Sha256::digest(authored)),
    })
}

struct ReviewCache {
    root: PathBuf,
    destination: PathBuf,
    identity: SnapshotIdentity,
    _guard: Guard,
}
impl ReviewCache {
    async fn prepare<B: NativeV2CliBackend>(
        &self,
        run: &RunCommand,
        runtime: &openengine_cluster_protocol::RuntimePlan,
        backend: &B,
    ) -> Result<PreparedReview, NativeV2CliError> {
        if let Some(mut prepared) = self.read()? {
            local_snapshot(run, &mut prepared, true).await?;
            return Ok(prepared);
        }
        let prepared = fresh_review(run, backend, self.identity.request.clone(), runtime).await?;
        self.write(&prepared)?;
        Ok(prepared)
    }

    async fn open(
        namespace: &str,
        key: &openengine_cluster_protocol::IdempotencyKey,
        identity: SnapshotIdentity,
    ) -> Result<Self, NativeV2CliError> {
        let root = crate::native_v2_cli::default_local_state_root()?.join("review-inputs");
        Self::open_at(root, cache_key(namespace, key)?, identity).await
    }
    async fn open_at(
        root: PathBuf,
        digest: String,
        identity: SnapshotIdentity,
    ) -> Result<Self, NativeV2CliError> {
        private_directory(&root).map_err(io)?;
        let destination = root.join(format!("{digest}.json"));
        let lock_path = root.join(format!("{digest}.lock"));
        let guard = tokio::task::spawn_blocking(move || lock_file(&lock_path))
            .await
            .map_err(|_| invalid("review retry lock is unavailable"))??;
        Ok(Self {
            root,
            destination,
            identity,
            _guard: guard,
        })
    }
    fn read(&self) -> Result<Option<PreparedReview>, NativeV2CliError> {
        let file = match private_file(&self.destination, FileAccess::Read) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io(error)),
        };
        let saved: SavedReview = serde_json::from_reader(file.take(4 * 1024 * 1024))
            .map_err(|_| invalid("saved review input is invalid"))?;
        if saved.identity != self.identity {
            return Err(invalid(
                "submission key already belongs to different review input or authored configuration",
            ));
        }
        Ok(Some(saved.prepared))
    }
    fn write(&self, prepared: &PreparedReview) -> Result<(), NativeV2CliError> {
        let bytes = serde_json::to_vec(&SavedReview {
            identity: self.identity.clone(),
            prepared: prepared.clone(),
        })
        .map_err(|_| invalid("resolved review input is invalid"))?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(invalid("resolved review input exceeds its bound"));
        }
        let temporary = self.root.join(format!("{}.tmp", uuid::Uuid::now_v7()));
        let file = private_file(&temporary, FileAccess::CreateNew).map_err(io)?;
        let result = crate::native_v2_cli::support::write_and_commit(
            file,
            &bytes,
            crate::native_v2_cli::support::CommitPaths {
                temporary: &temporary,
                destination: &self.destination,
                parent: &self.root,
            },
        );
        crate::native_v2_cli::support::cleanup_temporary(result, &temporary)
    }
}

#[cfg(test)]
#[path = "review_input/tests.rs"]
mod tests;

fn cache_key(
    namespace: &str,
    key: &openengine_cluster_protocol::IdempotencyKey,
) -> Result<String, NativeV2CliError> {
    let bytes = serde_json::to_vec(&(namespace, key))
        .map_err(|_| invalid("review retry identity is invalid"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn lock_file(path: &std::path::Path) -> Result<Guard, NativeV2CliError> {
    let file = private_file(path, FileAccess::ReadWrite).map_err(io)?;
    file.lock_exclusive().map_err(io)?;
    Ok(Guard(file))
}

fn invalid(message: &str) -> NativeV2CliError {
    NativeV2CliError::Usage(message.to_owned())
}
fn io(error: std::io::Error) -> NativeV2CliError {
    NativeV2CliError::Local(error.to_string())
}

async fn local_snapshot(
    run: &RunCommand,
    prepared: &mut PreparedReview,
    frozen: bool,
) -> Result<(), NativeV2CliError> {
    if run.target.is_some() {
        return Ok(());
    }
    let invoking = std::env::current_dir().map_err(io)?;
    let selected = run.cwd.as_deref().unwrap_or(&invoking);
    inspect_local_review(
        selected,
        &prepared.initial_input,
        &mut prepared.source,
        frozen,
    )
    .await
}

pub(super) async fn validate_local(
    params: &mut crate::native_v2_cli::PreparedRunRequest,
) -> Result<(), NativeV2CliError> {
    let invoking = std::env::current_dir().map_err(io)?;
    let selected = params.cwd.as_deref().unwrap_or(&invoking);
    let source = &mut params
        .source
        .as_mut()
        .ok_or_else(|| invalid("resolved local review source is unavailable"))?
        .resolved;
    inspect_local_review(selected, &params.intent.initial_input, source, true).await
}

async fn inspect_local_review(
    selected: &std::path::Path,
    input: &serde_json::Value,
    source: &mut openengine_cluster_protocol::ResolvedSource,
    frozen: bool,
) -> Result<(), NativeV2CliError> {
    let (workspace, _) =
        crate::native_v2_local::local_resolved_source(selected, std::path::Path::new("git"))
            .map_err(|error| invalid(&error.to_string()))?;
    let head = clean_local_head(&workspace).await?;
    require_review_head(input, &head)?;
    validate_local_repository(input, &local_git(&workspace, &["remote", "-v"]).await?)?;
    if frozen && source.revision.as_str() != head {
        return Err(invalid(
            "local review checkout HEAD changed after the review snapshot was prepared",
        ));
    }
    source.revision = openengine_cluster_protocol::SourceRevisionId::new(head)
        .map_err(|_| invalid("local review HEAD is invalid"))?;
    Ok(())
}

async fn clean_local_head(workspace: &std::path::Path) -> Result<String, NativeV2CliError> {
    let head = local_git(workspace, &["rev-parse", "--verify", "HEAD^{commit}"]).await?;
    let dirty = local_git(
        workspace,
        &["status", "--porcelain", "--untracked-files=no"],
    )
    .await?;
    if !dirty.is_empty() {
        return Err(invalid(
            "local review requires a checkout without tracked changes",
        ));
    }
    Ok(head)
}
fn require_review_head(input: &serde_json::Value, head: &str) -> Result<(), NativeV2CliError> {
    if input
        .pointer("/context/reviewScope/headRevision")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|expected| expected != head)
    {
        return Err(invalid(
            "local PR review checkout HEAD does not match the resolved PR head; select the matching worktree",
        ));
    }
    Ok(())
}

fn validate_local_repository(
    input: &serde_json::Value,
    remotes: &str,
) -> Result<(), NativeV2CliError> {
    let target =
        crate::native_v2_review::parse_review_url(input["url"].as_str().unwrap_or_default())
            .map_err(|error| invalid(&error.to_string()))?;

    let head_repository = input
        .pointer("/context/reviewScope/headRepository")
        .and_then(serde_json::Value::as_str);
    let matches = remotes
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .filter_map(crate::source_code_provider::adapters::core::github_repository_reference)
        .any(|repository| {
            repository.eq_ignore_ascii_case(&target.repository)
                || head_repository.is_some_and(|expected| repository.eq_ignore_ascii_case(expected))
        });
    if !matches {
        return Err(invalid(
            "local review checkout has no Git remote matching the issue or PR repository",
        ));
    }
    Ok(())
}

async fn local_git(workspace: &std::path::Path, args: &[&str]) -> Result<String, NativeV2CliError> {
    let mut command = tokio::process::Command::new("git");
    crate::native_v2_delivery::transport::remove_repository_environment(command.as_std_mut());
    command
        .current_dir(workspace)
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null());
    let output = crate::native_v2_delivery::command::capture_sensitive(
        &mut command,
        std::time::Duration::from_secs(10),
        &[],
    )
    .await
    .map_err(|_| invalid("local review Git inspection failed"))?;
    if output.diagnostic().exit_status != Some(0) || output.stdout_truncated() {
        return Err(invalid(
            "local review requires an exact committed Git checkout",
        ));
    }
    Ok(output.stdout().trim_end_matches(['\r', '\n']).to_owned())
}
