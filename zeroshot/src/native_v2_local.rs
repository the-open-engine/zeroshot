//! Trusted local-host composition for one native-v2 run in the invoking Git workspace.
//!
//! The local CLI snapshots repository identity, the attached target branch, and exact `HEAD`
//! before it starts a one-run controller. Provider values are selected only for names declared by
//! the submitted runtime plan. The candidate then runs as the invoking user in that same
//! workspace; no cleanup authority owns, resets, or removes workspace mutations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use openengine_cluster_protocol::{
    RunId, RunSubmission, RuntimePlan, SourceBranchId, SourceRepositoryId, SourceRevisionId,
    ResolvedSource,
};
use thiserror::Error;
use url::Url;

use crate::execution::process::HostedProcessPool;
use crate::native_v2_candidate::{
    NativeV2CandidateConfig, NativeV2CandidateError, NativeV2HarnessConfig,
    build_local_native_v2_candidate_with_github_token, build_local_owner_native_v2_candidate,
};
use crate::native_v2_claude::{ClaudeAdapterConfig, ClaudeAdapterConfigError, ClaudeProcessEnvironment};
use crate::native_v2_cli::PreparedRunRequest;
use crate::native_v2_codex::{NativeV2CodexConfig, NativeV2CodexUser};
use crate::native_v2_contract::AdmittedRun;
use crate::native_v2_copilot::{CopilotConfig, CopilotLocalUser};
use crate::native_v2_capsule::provider_process::{COPILOT_LOCAL_ENVIRONMENT, LocalHarnessEnvironment};
use crate::native_v2_delivery::{
    DeliveryTarget, GhCliAuthorityConfig, GhCliDeliveryAuthority, NativeV2DeliveryConfig,
};
use crate::native_v2_runner::{NativeNodeRunner, NodeRunner};
use crate::native_v2_supervisor::{RunEnvironment, RunEnvironmentError};

#[cfg(not(windows))]
const DEFAULT_SEARCH_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const MAX_GIT_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Debug, Error)]
pub enum LocalCompositionError {
    #[error("current Git workspace could not be resolved")]
    Workspace,
    #[error("current Git workspace must have an attached branch")]
    DetachedHead,
    #[error("current Git workspace must have a GitHub origin")]
    RepositoryIdentity,
    #[error("current Git resolved source is invalid")]
    ResolvedSource,
    #[error("local run identity could not be assigned")]
    RunIdentity,
    #[error(
        "setup and startup require a Docker target; local runs use the invoking machine environment"
    )]
    PreparationRequiresTarget,
    #[error(transparent)]
    RunEnvironment(#[from] RunEnvironmentError),
    #[error("local controller storage could not be prepared")]
    Storage,
    #[error("local harness environment could not be represented")]
    NativeEnvironment,
    #[error(transparent)]
    Claude(#[from] ClaudeAdapterConfigError),
    #[error(transparent)]
    Candidate(#[from] NativeV2CandidateError),
}

/// Host-assigned immutable inputs for one local controller process.
pub struct PreparedLocalRun {
    pub run_id: RunId,
    pub delivery_run_id: RunId,
    pub submission: RunSubmission,
    pub environment: RunEnvironment,
    pub github_token: Option<String>,
    pub workspace: PathBuf,
    pub native_environment: BTreeMap<String, String>,
}

pub struct LocalProcessCandidateRequest<'a> {
    pub admitted: &'a AdmittedRun,
    pub delivery_run_id: RunId,
    pub adopt_existing_delivery: bool,
    pub workspace: &'a Path,
    pub storage: &'a Path,
    pub github_token: Option<String>,
    pub native_environment: &'a BTreeMap<String, String>,
}

/// Snapshots local source and revalidates the request's exact runtime environment.
pub fn prepare_local_run(
    request: PreparedRunRequest,
    current_directory: &Path,
    git_program: &Path,
) -> Result<PreparedLocalRun, LocalCompositionError> {
    let PreparedRunRequest {
        run_id,
        intent,
        connections,
        github_token,
        source: _,
        profile: _,
    } = request;
    validate_local_environment(&intent.runtime)?;
    let environment = RunEnvironment::exact(&intent.runtime, connections)?;
    let (workspace, source) = local_resolved_source(current_directory, git_program)?;
    let native_environment = capture_local_native_environment(current_directory)?;
    Ok(PreparedLocalRun {
        delivery_run_id: run_id.clone(),
        run_id,
        submission: RunSubmission {
            title: intent.title,
            graph: intent.graph,
            initial_input: intent.initial_input,
            runtime: intent.runtime,
            source,
            submission_key: intent.submission_key,
        },
        environment,
        github_token,
        workspace,
        native_environment,
    })
}

pub(crate) fn validate_local_environment(
    runtime: &RuntimePlan,
) -> Result<(), LocalCompositionError> {
    if runtime.environment().is_some_and(|environment| {
        environment.setup.is_some()
            || environment.startup.is_some()
            || !environment.connections.is_empty()
    }) {
        return Err(LocalCompositionError::PreparationRequiresTarget);
    }
    Ok(())
}

pub(crate) fn capture_local_native_environment(
    invoking_directory: &Path,
) -> Result<BTreeMap<String, String>, LocalCompositionError> {
    let mut environment = LocalHarnessEnvironment::new(
        crate::native_v2_capsule::provider_process::current_process_environment(),
    );
    for name in [
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "COPILOT_HOME",
        "COPILOT_PROVIDERS_CONFIG",
    ] {
        let Some(value) = environment.get_mut(name) else {
            continue;
        };
        let path = PathBuf::from(&*value);
        if path.is_relative() {
            *value = invoking_directory
                .join(path)
                .into_os_string()
                .into_string()
                .map_err(|_| LocalCompositionError::NativeEnvironment)?;
        }
    }
    Ok(environment.into_values())
}

pub(crate) fn local_resolved_source(
    current_directory: &Path,
    git_program: &Path,
) -> Result<(PathBuf, ResolvedSource), LocalCompositionError> {
    let workspace = PathBuf::from(git_line(
        git_program,
        current_directory,
        &["rev-parse", "--show-toplevel"],
    )?);
    let workspace =
        std::fs::canonicalize(workspace).map_err(|_| LocalCompositionError::Workspace)?;
    let branch = git_line(
        git_program,
        &workspace,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .map_err(|_| LocalCompositionError::DetachedHead)?;
    let revision = git_line(
        git_program,
        &workspace,
        &["rev-parse", "--verify", "HEAD^{commit}"],
    )?;
    let origin = git_line(
        git_program,
        &workspace,
        &["config", "--get", "remote.origin.url"],
    )
    .map_err(|_| LocalCompositionError::RepositoryIdentity)?;
    let repository = github_repository(&origin).ok_or(LocalCompositionError::RepositoryIdentity)?;
    let source = ResolvedSource {
        repository: SourceRepositoryId::new(repository)
            .map_err(|_| LocalCompositionError::ResolvedSource)?,
        branch: SourceBranchId::new(branch).map_err(|_| LocalCompositionError::ResolvedSource)?,
        revision: SourceRevisionId::new(revision)
            .map_err(|_| LocalCompositionError::ResolvedSource)?,
    };
    Ok((workspace, source))
}

fn git_line(
    git_program: &Path,
    workspace: &Path,
    arguments: &[&str],
) -> Result<String, LocalCompositionError> {
    let output = Command::new(git_program)
        .arg("-C")
        .arg(workspace)
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| LocalCompositionError::Workspace)?;
    if !output.status.success() || output.stdout.len() > MAX_GIT_OUTPUT_BYTES {
        return Err(LocalCompositionError::Workspace);
    }
    let value = String::from_utf8(output.stdout).map_err(|_| LocalCompositionError::Workspace)?;
    let value = value.trim_end_matches(['\r', '\n']);
    if value.is_empty() || value.contains(['\r', '\n', '\0']) {
        return Err(LocalCompositionError::Workspace);
    }
    Ok(value.to_owned())
}

fn github_repository(origin: &str) -> Option<String> {
    let path = github_remote_path(origin)?;
    let path = path
        .trim_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_matches('/'));
    let mut segments = path.split('/');
    let owner = segments.next()?;
    let repository = segments.next()?;
    if owner.is_empty() || repository.is_empty() || segments.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{repository}"))
}

fn github_remote_path(origin: &str) -> Option<String> {
    match Url::parse(origin) {
        Ok(url) => url
            .host_str()
            .filter(|host| host.eq_ignore_ascii_case("github.com"))
            .map(|_| url.path().to_owned()),
        Err(_) => {
            let (authority, path) = origin.split_once(':')?;
            let host = authority
                .rsplit_once('@')
                .map_or(authority, |(_, host)| host);
            host.eq_ignore_ascii_case("github.com")
                .then(|| path.to_owned())
        }
    }
}

/// Builds a local-user process candidate rooted in the existing workspace.
///
/// `storage` owns only provider session homes and controller state. No cleanup object is returned
/// because the existing workspace and every mutation within it remain user-owned.
pub fn build_local_process_candidate(
    request: LocalProcessCandidateRequest<'_>,
) -> Result<Arc<dyn NodeRunner>, LocalCompositionError> {
    build_local_candidate_config(request, false).map(|candidate| Arc::new(candidate) as _)
}

pub(crate) fn build_local_owner_process_candidate(
    request: LocalProcessCandidateRequest<'_>,
) -> Result<Arc<NativeNodeRunner>, LocalCompositionError> {
    build_local_candidate_config(request, true).map(Arc::new)
}

fn build_local_candidate_config(
    request: LocalProcessCandidateRequest<'_>,
    owner_scoped: bool,
) -> Result<NativeNodeRunner, LocalCompositionError> {
    validate_local_environment(&request.admitted.runtime)?;
    let LocalProcessCandidateRequest {
        admitted,
        delivery_run_id,
        adopt_existing_delivery,
        workspace,
        storage,
        github_token,
        native_environment,
    } = request;
    let runtime_home = storage.join("runtime");
    prepare_private_directory(&runtime_home)?;
    let harness = local_harness(admitted, workspace, &runtime_home, native_environment)?;
    let target = DeliveryTarget::new(
        admitted.source.repository.as_str(),
        admitted.source.branch.as_str(),
        admitted.source.revision.as_str(),
    )
    .map_err(|_| LocalCompositionError::ResolvedSource)?;
    let mut github_config = GhCliAuthorityConfig::hosted(runtime_home);
    github_config.git_program = PathBuf::from("git");
    github_config.gh_program = PathBuf::from("gh");
    let config = NativeV2CandidateConfig {
        harness,
        delivery: NativeV2DeliveryConfig {
            delivery_run_id,
            adopt_existing_delivery,
            git_identity: None,
            workspace: workspace.to_owned(),
            git_program: PathBuf::from("git"),
            target,
            poll: Default::default(),
        },
        github: Arc::new(GhCliDeliveryAuthority::new(github_config)),
    };
    if owner_scoped {
        build_local_owner_native_v2_candidate(admitted, config).map_err(Into::into)
    } else {
        build_local_native_v2_candidate_with_github_token(
            admitted,
            config,
            github_token.map(Arc::<str>::from),
        )
        .map_err(Into::into)
    }
}

fn local_harness(
    admitted: &AdmittedRun,
    workspace: &Path,
    runtime_home: &Path,
    native_environment: &BTreeMap<String, String>,
) -> Result<NativeV2HarnessConfig, LocalCompositionError> {
    let native_environment = LocalHarnessEnvironment::new(native_environment.clone());
    let local_command_environment = native_environment.clone().into_values();
    let search_path = native_environment
        .get("PATH")
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| default_search_path(&native_environment));
    let local_home = current_user_home(&native_environment);
    let process_pool = HostedProcessPool::hosted_default();
    let harness = match &admitted.runtime {
        RuntimePlan::Copilot { .. } => NativeV2HarnessConfig::Copilot(CopilotConfig {
            executable: PathBuf::from("copilot"),
            workspace: workspace.to_owned(),
            runtime_home: runtime_home.to_owned(),
            local_user: local_home.clone().map(|home| CopilotLocalUser {
                copilot_home: native_environment
                    .get("COPILOT_HOME")
                    .filter(|value| !value.is_empty())
                    .map_or_else(|| home.join(".copilot"), PathBuf::from),
                home,
            }),
            base_environment: native_environment.selected(COPILOT_LOCAL_ENVIRONMENT),
            local_command_environment,
            search_path: search_path.clone(),
            process_pool,
        }),
        RuntimePlan::Codex { provider, .. } => NativeV2HarnessConfig::Codex(NativeV2CodexConfig {
            base_environment: Default::default(),
            provider: *provider,
            executable: PathBuf::from("codex"),
            workspace: workspace.to_owned(),
            runtime_home: runtime_home.to_owned(),
            local_user: local_home.clone().map(|home| NativeV2CodexUser {
                codex_home: native_environment
                    .get("CODEX_HOME")
                    .filter(|value| !value.is_empty())
                    .map_or_else(|| home.join(".codex"), PathBuf::from),
                home,
            }),
            native_environment: native_environment.clone(),
            search_path: search_path.clone(),
            process_pool,
        }),
        RuntimePlan::Claude { provider, .. } => {
            NativeV2HarnessConfig::Claude(ClaudeAdapterConfig {
                provider: *provider,
                executable: "claude".to_owned(),
                prefix_arguments: Vec::new(),
                workspace: workspace.to_owned(),
                runtime_home: runtime_home.to_owned(),
                local_user_home: local_home,
                native_environment: native_environment.clone(),
                base_environment: local_claude_environment(&search_path, &native_environment)?,
                process_pool,
            })
        }
    };
    Ok(harness)
}

fn current_user_home(environment: &LocalHarnessEnvironment) -> Option<PathBuf> {
    environment
        .user_home()
        .map(PathBuf::from)
        .or_else(crate::execution::platform::user_home)
}

fn default_search_path(environment: &LocalHarnessEnvironment) -> String {
    #[cfg(windows)]
    {
        environment
            .get("SystemRoot")
            .filter(|value| !value.is_empty())
            .map(|root| format!(r"{root}\System32;{root}"))
            .unwrap_or_else(|| r"C:\Windows\System32;C:\Windows".to_owned())
    }
    #[cfg(not(windows))]
    {
        let _ = environment;
        DEFAULT_SEARCH_PATH.to_owned()
    }
}

fn local_claude_environment(
    search_path: &str,
    native_environment: &LocalHarnessEnvironment,
) -> Result<ClaudeProcessEnvironment, ClaudeAdapterConfigError> {
    let mut base_environment = BTreeMap::from([("PATH".to_owned(), search_path.to_owned())]);
    for name in ["LANG", "LC_ALL", "TERM", "TMPDIR"] {
        if let Some(value) = native_environment.get(name) {
            base_environment.insert(name.to_owned(), value.clone());
        }
    }
    ClaudeProcessEnvironment::new(base_environment)
}

fn prepare_private_directory(path: &Path) -> Result<(), LocalCompositionError> {
    crate::execution::platform::private_directory(path).map_err(|_| LocalCompositionError::Storage)
}

#[cfg(test)]
#[path = "native_v2_local/tests.rs"]
mod contract_tests;
