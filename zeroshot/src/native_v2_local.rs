//! Trusted local-host composition for one native-v2 run in a selected folder.
//!
//! Repository metadata is optional until the graph asks for Git delivery. Provider values are
//! selected only for admitted requirements. The candidate runs as the invoking user; the folder
//! and its mutations remain user-owned.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use openengine_cluster_protocol::{
    GitTransportCredentials, RunId, RunSubmission, RuntimeLane, RuntimePlan, ResolvedSource,
};
use thiserror::Error;

use crate::execution::process::HostedProcessPool;
use crate::native_v2_candidate::{
    NativeV2CandidateConfig, NativeV2CandidateDelivery, NativeV2CandidateError,
    NativeV2HarnessConfig, TrustedDeliveryCapabilities,
    build_local_native_v2_candidate_with_authority, build_local_owner_native_v2_candidate,
};
use crate::native_v2_claude::{ClaudeAdapterConfig, ClaudeAdapterConfigError};
use crate::native_v2_cli::PreparedRunRequest;
use crate::native_v2_codex::{NativeV2CodexConfig, NativeV2CodexUser};
use crate::native_v2_contract::AdmittedRun;
use crate::native_v2_capsule::HarnessProcessEnvironment;
use crate::native_v2_copilot::{CopilotConfig, CopilotLocalUser};
use crate::native_v2_pi::{PiAdapterConfigError, PiConfig};
use crate::native_v2_capsule::provider_process::{COPILOT_LOCAL_ENVIRONMENT, LocalHarnessEnvironment};
use crate::native_v2_delivery::{NativeV2DeliveryConfig, TrustedGitHubTokenFile};
use crate::native_v2_delivery::validation::TrustedDeliveryValidator;
mod token_file;
pub(crate) use token_file::{take_github_authority, take_delivery_validator};
use crate::source_code_provider::adapters::core::{SourceDeliveryFactoryRequest, delivery_authority};
use crate::native_v2_runner::{NativeNodeRunner, NodeRunner};
use crate::native_v2_supervisor::{RunEnvironment, RunEnvironmentError};

#[cfg(not(windows))]
const DEFAULT_SEARCH_PATH: &str = "/usr/local/bin:/usr/bin:/bin";
const MAX_GIT_OUTPUT_BYTES: usize = 16 * 1024;

mod source;
use source::resolved_git_source;
pub(crate) use source::validate_remote_base_branch;
mod workspace;
pub(crate) use workspace::validate_workspace_state;
use workspace::resolve_workspace;

#[derive(Debug, Error)]
pub enum LocalCompositionError {
    #[error(transparent)]
    ProcessEnvironment(#[from] crate::native_v2_capsule::ProcessEnvironmentError),
    #[error("current working folder could not be resolved")]
    Workspace,
    #[error(
        "workspace {workspace:?} overlaps state {state:?}; choose another folder or external ZEROSHOT_STATE_DIR"
    )]
    WorkspaceStateOverlap { workspace: PathBuf, state: PathBuf },
    #[error("current Git workspace must have an attached branch")]
    DetachedHead,
    #[error("Git delivery needs a credential-free HTTPS or SSH remote")]
    RepositoryIdentity,
    #[error("multiple eligible Git remotes were found; configure a branch upstream")]
    AmbiguousRemote,
    #[error("Git delivery requires a committed repository source")]
    DeliverySourceRequired,
    #[error(
        "remote does not have target branch `{branch}`; set the intended upstream (for example `git branch --set-upstream-to=origin/main`) or create the worktree from `origin/main`"
    )]
    MissingRemoteBase { branch: String },
    #[error(
        "could not verify target branch `{branch}` on the remote; check remote connectivity and credentials"
    )]
    RemoteBaseCheck { branch: String },
    #[error("Git delivery credentials are invalid")]
    DeliveryCredentials,
    #[error("current Git resolved source is invalid")]
    ResolvedSource,
    #[error("local run identity could not be assigned")]
    RunIdentity,
    #[error(
        "setup and startup require a Docker target; local runs use the invoking machine environment"
    )]
    PreparationRequiresTarget,
    #[error(transparent)]
    SourceDelivery(#[from] crate::native_v2_admission::NativeV2AdmissionError),
    #[error(transparent)]
    RunEnvironment(#[from] RunEnvironmentError),
    #[error("local controller storage could not be prepared")]
    Storage,
    #[error("trusted local delivery validator configuration is invalid")]
    DeliveryValidator,
    #[error("trusted local GitHub credential configuration is invalid")]
    GitHubTokenFile,
    #[error("local harness environment could not be represented")]
    NativeEnvironment,
    #[error("lane {lane} needs the `{executable}` executable on PATH")]
    MissingLaneExecutable {
        lane: RuntimeLane,
        executable: &'static str,
    },
    #[error("Git delivery needs the `{executable}` executable on PATH")]
    MissingDeliveryExecutable { executable: &'static str },
    #[error(transparent)]
    Claude(#[from] ClaudeAdapterConfigError),
    #[error(transparent)]
    Pi(#[from] PiAdapterConfigError),
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
    pub github_token_file: Option<TrustedGitHubTokenFile>,
    pub delivery_validator: Option<TrustedDeliveryValidator>,
    pub git_credentials: Option<GitTransportCredentials>,
    pub workspace: PathBuf,
    pub native_environment: BTreeMap<String, String>,
}

pub struct LocalProcessCandidateRequest<'a> {
    pub admitted: &'a AdmittedRun,
    pub delivery_run_id: RunId,
    pub adopt_existing_delivery: bool,
    pub workspace: &'a Path,
    pub storage: &'a Path,
    pub memory_storage: Option<&'a Path>,
    pub github_token: Option<String>,
    pub github_token_file: Option<TrustedGitHubTokenFile>,
    pub delivery_validator: Option<TrustedDeliveryValidator>,
    pub git_credentials: Option<GitTransportCredentials>,
    pub native_environment: &'a BTreeMap<String, String>,
}

/// Snapshots local source and revalidates the request's exact runtime environment.
pub fn prepare_local_run(
    request: PreparedRunRequest,
    current_directory: &Path,
    git_program: &Path,
) -> Result<PreparedLocalRun, LocalCompositionError> {
    validate_local_environment(request.intent.environment.as_ref())?;
    let (workspace, source) = prepare_local_source(&request, current_directory, git_program)?;
    let submission = local_submission(&request.intent, source);
    let environment = RunEnvironment::exact_for_submission(&submission, request.connections)?;
    let mut native_environment = capture_local_native_environment(current_directory)?;
    let (github_token, github_token_file) = take_github_authority(
        &mut native_environment,
        request.github_token,
        current_directory,
        &workspace,
    )?;
    let delivery_validator = take_delivery_validator(&mut native_environment, &workspace)?;
    Ok(PreparedLocalRun {
        delivery_run_id: request.run_id.clone(),
        run_id: request.run_id,
        submission,
        environment,
        github_token,
        github_token_file,
        delivery_validator,
        git_credentials: request.git_credentials,
        workspace,
        native_environment,
    })
}

pub(crate) fn prepare_local_source(
    request: &PreparedRunRequest,
    invoking_directory: &Path,
    git_program: &Path,
) -> Result<(PathBuf, Option<ResolvedSource>), LocalCompositionError> {
    let selected = request.cwd.as_deref().unwrap_or(invoking_directory);
    let selected = if selected.is_absolute() {
        selected.to_owned()
    } else {
        invoking_directory.join(selected)
    };
    let workspace = resolve_workspace(&selected, git_program)?;
    let required =
        !crate::native_v2_admission::delivery_capabilities(&request.intent.graph.root).is_empty();
    let source = match &request.source {
        Some(source) => Some(source.resolved.clone()),
        None if required => Some(resolved_git_source(
            &workspace,
            git_program,
            request.source_connection.clone(),
        )?),
        None => resolved_git_source(&workspace, git_program, None).ok(),
    };
    crate::native_v2_admission::validate_source_delivery(source.as_ref(), &request.intent.graph)
        .map_err(LocalCompositionError::from)?;
    Ok((workspace, source))
}

pub(crate) fn local_submission(
    intent: &crate::native_v2_cli::TargetRunIntent,
    source: Option<ResolvedSource>,
) -> RunSubmission {
    RunSubmission {
        title: intent.title.clone(),
        graph: intent.graph.clone(),
        initial_input: intent.initial_input.clone(),
        runtime: intent.runtime.clone(),
        environment: intent.environment.clone(),
        source,
        submission_key: intent.submission_key.clone(),
    }
}

pub(crate) fn validate_local_environment(
    environment: Option<&openengine_cluster_protocol::RuntimeEnvironment>,
) -> Result<(), LocalCompositionError> {
    if environment.is_some_and(|environment| {
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
    token_file::validate_caller_authority(|name| std::env::var_os(name))?;
    let mut environment = LocalHarnessEnvironment::new(
        crate::native_v2_capsule::provider_process::current_process_environment(),
    );
    resolve_relative_home_overrides(&mut environment, invoking_directory)?;
    Ok(environment.into_values())
}

/// Resolves every harness home override that the invoking shell may have set relatively against
/// the invoking directory, before the controller detaches. The variable is forwarded to the
/// harness child, whose working directory differs, so leaving it relative would change what it
/// points at.
fn resolve_relative_home_overrides(
    environment: &mut LocalHarnessEnvironment,
    invoking_directory: &Path,
) -> Result<(), LocalCompositionError> {
    let user_home = current_user_home(environment);
    for name in [
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "COPILOT_HOME",
        "COPILOT_PROVIDERS_CONFIG",
        "PI_CODING_AGENT_DIR",
    ] {
        let Some(value) = environment.get_mut(name) else {
            continue;
        };
        // Pi owns file-URL decoding, including platform rules and percent escapes.
        if name == "PI_CODING_AGENT_DIR" && value.starts_with("file://") {
            continue;
        }
        let path = if name == "PI_CODING_AGENT_DIR" {
            pi_agent_directory_path(value, user_home.as_deref())?
        } else {
            PathBuf::from(&*value)
        };
        let path = if path.is_relative() {
            invoking_directory.join(path)
        } else {
            path
        };
        *value = path
            .into_os_string()
            .into_string()
            .map_err(|_| LocalCompositionError::NativeEnvironment)?;
    }
    Ok(())
}

/// Pi expands a leading home shorthand before resolving its agent directory. Resolve it against
/// the captured user's home before detachment instead of treating the tilde as a workspace name.
fn pi_agent_directory_path(
    value: &str,
    user_home: Option<&Path>,
) -> Result<PathBuf, LocalCompositionError> {
    let relative = if value == "~" {
        Some("")
    } else {
        value
            .strip_prefix("~/")
            .or_else(|| value.strip_prefix("~\\").filter(|_| cfg!(windows)))
    };
    match relative {
        Some(relative) => Ok(user_home
            .ok_or(LocalCompositionError::NativeEnvironment)?
            .join(relative)),
        None => Ok(PathBuf::from(value)),
    }
}

/// A branch-independent identity proof for retrospective local history selection.
/// Runtime source admission still requires an exact attached source when delivery needs one.
pub(crate) fn local_memory_remote(
    workspace: &Path,
) -> Option<openengine_cluster_protocol::GitRemoteUrl> {
    source::select_remote(workspace, Path::new("git"), None).ok()
}

pub(crate) fn local_resolved_source(
    current_directory: &Path,
    git_program: &Path,
) -> Result<(PathBuf, Option<ResolvedSource>), LocalCompositionError> {
    let workspace = resolve_workspace(current_directory, git_program)?;
    let source = resolved_git_source(&workspace, git_program, None).ok();
    Ok((workspace, source))
}

fn git_line(
    git_program: &Path,
    workspace: &Path,
    arguments: &[&str],
) -> Result<String, LocalCompositionError> {
    let value = git_output(git_program, workspace, arguments)?;
    if value.is_empty() || value.contains(['\r', '\n', '\0']) {
        return Err(LocalCompositionError::Workspace);
    }
    Ok(value)
}

fn git_output(
    git_program: &Path,
    workspace: &Path,
    arguments: &[&str],
) -> Result<String, LocalCompositionError> {
    let mut command = Command::new(git_program);
    crate::native_v2_delivery::transport::remove_repository_environment(&mut command);
    let output = command
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
    Ok(value.trim_end_matches(['\r', '\n']).to_owned())
}

/// Builds a local-user process candidate rooted in the existing workspace.
///
/// `storage` owns only provider session homes and controller state. No cleanup object is returned
/// because the existing workspace and every mutation within it remain user-owned.
pub fn build_local_process_candidate(
    request: LocalProcessCandidateRequest<'_>,
) -> Result<Arc<dyn NodeRunner>, LocalCompositionError> {
    let admitted = request.admitted;
    let prepared = crate::active_memory::runtime::PreparedActiveMemory::prepare(
        crate::active_memory::runtime::MemoryPreparation {
            admitted,
            workspace: request.workspace,
            storage: request.memory_storage,
        },
    );
    let hooks = prepared
        .as_ref()
        .ok()
        .and_then(|memory| memory.as_ref())
        .map(|memory| memory.hooks.clone());
    let runner = build_local_candidate_config(request, false, hooks)?;
    Ok(match prepared {
        Ok(Some(memory)) => memory.wrap(admitted, runner),
        Ok(None) => Arc::new(runner),
        Err(_) => crate::active_memory::runtime::unavailable_memory(Arc::new(runner)),
    })
}

pub(crate) fn build_local_owner_process_candidate(
    request: LocalProcessCandidateRequest<'_>,
) -> Result<Arc<NativeNodeRunner>, LocalCompositionError> {
    build_local_candidate_config(request, true, None).map(Arc::new)
}

fn build_local_candidate_config(
    request: LocalProcessCandidateRequest<'_>,
    owner_scoped: bool,
    active_memory: Option<Arc<crate::active_memory::hooks::ActiveMemoryHooks>>,
) -> Result<NativeNodeRunner, LocalCompositionError> {
    validate_local_environment(request.admitted.environment.as_ref())?;
    let LocalProcessCandidateRequest {
        admitted,
        delivery_run_id,
        adopt_existing_delivery,
        workspace,
        storage,
        memory_storage: _,
        github_token,
        github_token_file,
        delivery_validator,
        git_credentials,
        native_environment,
    } = request;
    let runtime_home = storage.join("runtime");
    prepare_private_directory(&runtime_home)?;
    let lanes = local_lanes(admitted, workspace, &runtime_home, native_environment)?;
    let required_capabilities =
        crate::native_v2_admission::delivery_capabilities(&admitted.graph.root);
    let delivery = if required_capabilities.is_empty() {
        None
    } else {
        let source = admitted
            .source
            .as_ref()
            .ok_or(LocalCompositionError::DeliverySourceRequired)?;
        let mode = crate::native_v2_admission::delivery_mode(&admitted.graph.root)
            .ok_or(LocalCompositionError::DeliverySourceRequired)?;
        let target = crate::native_v2_delivery::source_target(source, mode)
            .map_err(|_| LocalCompositionError::ResolvedSource)?;
        let programs = local_delivery_programs(
            source,
            &required_capabilities,
            native_environment,
            workspace,
        )?;
        let authority = delivery_authority(SourceDeliveryFactoryRequest {
            source,
            workspace,
            git_program: &programs.git,
            gh_program: &programs.gh,
            home_directory: &runtime_home,
            private_directory: &runtime_home,
            git_identity: None,
            local_environment: Some(native_environment.clone()),
            required_capabilities: &required_capabilities,
            operator_diagnostics: None,
        })
        .map_err(|_| LocalCompositionError::ResolvedSource)?;
        Some(NativeV2CandidateDelivery {
            config: NativeV2DeliveryConfig {
                delivery_run_id,
                adopt_existing_delivery,
                git_identity: None,
                workspace: workspace.to_owned(),
                git_program: programs.git,
                target,
                poll: Default::default(),
            },
            authority,
            git_credentials,
        })
    };
    let config = NativeV2CandidateConfig {
        active_memory,
        lanes,
        workspace: workspace.to_owned(),
        delivery,
    };
    if owner_scoped {
        build_local_owner_native_v2_candidate(admitted, config).map_err(Into::into)
    } else {
        build_local_native_v2_candidate_with_authority(
            admitted,
            config,
            TrustedDeliveryCapabilities {
                token: github_token.map(Arc::<str>::from),
                token_file: github_token_file,
                validator: delivery_validator,
            },
        )
        .map_err(Into::into)
    }
}

struct LocalLaneInputs<'a> {
    workspace: &'a Path,
    runtime_home: &'a Path,
    native_environment: LocalHarnessEnvironment,
    local_command_environment: BTreeMap<String, String>,
    search_path: String,
    local_home: Option<PathBuf>,
    process_pool: HostedProcessPool,
}

fn local_lanes(
    admitted: &AdmittedRun,
    workspace: &Path,
    runtime_home: &Path,
    native_environment: &BTreeMap<String, String>,
) -> Result<Vec<NativeV2HarnessConfig>, LocalCompositionError> {
    let native_environment =
        LocalHarnessEnvironment::new(token_file::provider_environment(native_environment));
    let inputs = LocalLaneInputs {
        workspace,
        runtime_home,
        local_command_environment: native_environment.clone().into_values(),
        search_path: local_search_path(&native_environment),
        local_home: current_user_home(&native_environment),
        process_pool: HostedProcessPool::hosted_default(),
        native_environment,
    };
    admitted
        .runtime
        .lanes()
        .into_iter()
        .map(|lane| local_lane(lane, &inputs))
        .collect()
}

fn local_lane(
    lane: RuntimeLane,
    inputs: &LocalLaneInputs<'_>,
) -> Result<NativeV2HarnessConfig, LocalCompositionError> {
    let LocalLaneInputs {
        workspace,
        runtime_home,
        ref native_environment,
        ref local_command_environment,
        ref search_path,
        ref local_home,
        process_pool,
    } = *inputs;
    let harness = match lane {
        RuntimeLane::Copilot { .. } => NativeV2HarnessConfig::Copilot(CopilotConfig {
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
            local_command_environment: local_command_environment.clone(),
            search_path: search_path.clone(),
            process_pool,
        }),
        RuntimeLane::Codex { provider } => NativeV2HarnessConfig::Codex(NativeV2CodexConfig {
            base_environment: Default::default(),
            provider,
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
        RuntimeLane::Claude { provider } => NativeV2HarnessConfig::Claude(ClaudeAdapterConfig {
            provider,
            executable: "claude".to_owned(),
            prefix_arguments: Vec::new(),
            workspace: workspace.to_owned(),
            runtime_home: runtime_home.to_owned(),
            local_user_home: local_home.clone(),
            native_environment: native_environment.clone(),
            base_environment: local_process_environment(search_path, native_environment)?,
            process_pool,
        }),
        RuntimeLane::Pi { provider } => NativeV2HarnessConfig::Pi(PiConfig {
            provider,
            executable: "pi".to_owned(),
            prefix_arguments: Vec::new(),
            workspace: workspace.to_owned(),
            runtime_home: runtime_home.to_owned(),
            local_user_home: local_home.clone(),
            native_environment: native_environment.clone(),
            base_environment: local_process_environment(search_path, native_environment)?,
            process_pool,
        }),
    };
    Ok(harness)
}

pub(crate) fn local_search_path(environment: &LocalHarnessEnvironment) -> String {
    environment
        .get("PATH")
        .filter(|value| !value.is_empty())
        .cloned()
        .unwrap_or_else(|| default_search_path(environment))
}

pub(crate) fn search_path_depends_on_working_directory(
    native_environment: &BTreeMap<String, String>,
) -> bool {
    let search_path = local_search_path(&LocalHarnessEnvironment::new(native_environment.clone()));
    std::env::split_paths(&search_path).any(|entry| entry.is_relative())
}

fn find_local_executable(
    executable: &str,
    native_environment: &BTreeMap<String, String>,
    working_directory: &Path,
) -> Option<PathBuf> {
    let search_path = local_search_path(&LocalHarnessEnvironment::new(native_environment.clone()));
    let lookup = BTreeMap::from([("PATH".to_owned(), search_path)]);
    crate::execution::platform::find_executable(executable, &lookup, working_directory)
}

/// Checks that each lane's harness executable is on the search path its adapter spawns with.
pub(crate) fn check_lane_executables(
    runtime: &RuntimePlan,
    native_environment: &BTreeMap<String, String>,
    working_directory: &Path,
) -> Result<(), LocalCompositionError> {
    for lane in runtime.lanes() {
        let executable = lane.harness_name();
        if find_local_executable(executable, native_environment, working_directory).is_none() {
            return Err(LocalCompositionError::MissingLaneExecutable { lane, executable });
        }
    }
    Ok(())
}

struct LocalDeliveryPrograms {
    git: PathBuf,
    gh: PathBuf,
}

/// Pins delivery's programs to the files the captured search path finds. Delivery clears the
/// environment of its Git and GitHub CLI processes, so a bare name would search only the system's
/// default directories.
fn local_delivery_programs(
    source: &ResolvedSource,
    required: &[crate::source_code_provider::SourceCapability],
    native_environment: &BTreeMap<String, String>,
    working_directory: &Path,
) -> Result<LocalDeliveryPrograms, LocalCompositionError> {
    let find = |executable| {
        find_local_executable(executable, native_environment, working_directory)
            .ok_or(LocalCompositionError::MissingDeliveryExecutable { executable })
    };
    // Other providers never start the GitHub CLI.
    let gh = if crate::source_code_provider::adapters::core::delivery_uses_github_cli(
        source, required,
    ) {
        find("gh")?
    } else {
        PathBuf::from("gh")
    };
    Ok(LocalDeliveryPrograms {
        git: find("git")?,
        gh,
    })
}

/// Checks that the programs the submission's delivery runs are on the captured search path.
pub(crate) fn check_delivery_executables(
    submission: &RunSubmission,
    native_environment: &BTreeMap<String, String>,
    working_directory: &Path,
) -> Result<(), LocalCompositionError> {
    let required = crate::native_v2_admission::delivery_capabilities(&submission.graph.root);
    match &submission.source {
        Some(source) if !required.is_empty() => {
            local_delivery_programs(source, &required, native_environment, working_directory)
                .map(drop)
        }
        _ => Ok(()),
    }
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

fn local_process_environment(
    search_path: &str,
    native_environment: &LocalHarnessEnvironment,
) -> Result<HarnessProcessEnvironment, crate::native_v2_capsule::ProcessEnvironmentError> {
    let mut base_environment = BTreeMap::from([("PATH".to_owned(), search_path.to_owned())]);
    for name in ["LANG", "LC_ALL", "TERM", "TMPDIR"] {
        if let Some(value) = native_environment.get(name) {
            base_environment.insert(name.to_owned(), value.clone());
        }
    }
    HarnessProcessEnvironment::new(base_environment)
}

fn prepare_private_directory(path: &Path) -> Result<(), LocalCompositionError> {
    crate::execution::platform::private_directory(path).map_err(|_| LocalCompositionError::Storage)
}

#[cfg(test)]
#[path = "native_v2_local/tests.rs"]
mod contract_tests;
