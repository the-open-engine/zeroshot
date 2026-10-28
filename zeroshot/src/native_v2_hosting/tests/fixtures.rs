use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use openengine_cluster_protocol::{
    DeclaredConnections, DeclaredEnvironment, GraphSpec, IdempotencyKey, NodeName, RunSize,
    RunTitle, RuntimePlan, SourceBranchId, SourceRepositoryId, SourceRevisionId, ResolvedSource,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use crate::execution::SessionScope;
use crate::execution::process::HostedProcessPool;
use crate::native_v2_candidate::test_support::{TestDirectory, full_graph, success_node};
use crate::native_v2_capsule::CapsuleFilesystem;
use crate::native_v2_claude::ClaudeProcessEnvironment;
use crate::native_v2_pi::PiProcessEnvironment;
use crate::native_v2_platform::CapsuleAllocationUnavailable;
use crate::native_v2_contract::{
    ClaudeProvider, EnvironmentVariableName, NodeRuntimeBinding, RunSubmission,
};
use crate::worker_catalog::{self, ReasoningEffort};

use super::super::ProductionHostingConfig;
use super::super::allocator::ProductionCapsuleAllocator;
use super::super::ProductionHostingError;
use crate::native_v2_target_authority::OperatorDiagnosticStore;
use std::sync::Arc;

pub(super) struct RepositoryFixture {
    _root: TestDirectory,
    pub(super) remote: PathBuf,
    pub(super) main_revision: String,
    pub(super) feature_revision: String,
}

impl RepositoryFixture {
    pub(super) fn new() -> Self {
        let root = TestDirectory::new("hosting-repository");
        let seed = root.path().join("seed");
        let remote = root.path().join("remote.git");
        git(root.path(), &["init", "--initial-branch=main", text(&seed)]);
        fs::write(seed.join("README.md"), "base\n").assert_value_with("write base");
        git(&seed, &["add", "README.md"]);
        commit(&seed, "base");
        let main_revision = git_output(&seed, &["rev-parse", "HEAD"]);
        git(&seed, &["switch", "-c", "feature"]);
        fs::write(seed.join("feature.txt"), "feature\n").assert_value_with("write feature");
        git(&seed, &["add", "feature.txt"]);
        commit(&seed, "feature");
        let feature_revision = git_output(&seed, &["rev-parse", "HEAD"]);
        git(&seed, &["switch", "main"]);
        git(
            root.path(),
            &["clone", "--bare", text(&seed), text(&remote)],
        );
        git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
        if unsafe { libc::geteuid() } == 0 {
            // The local upload-pack reads this directory as the contained checkout writer.
            let writer = test_process_pool()
                .active_run_slot(
                    0,
                    openengine_cluster_server::graph_verifier::FULL_V1_MAX_NODE_EXECUTIONS,
                )
                .assert_value()
                .identity(crate::execution::process::HostedProcessScope::Writer)
                .assert_value();
            std::os::unix::fs::chown(&remote, Some(writer.uid()), Some(writer.gid()))
                .assert_value();
        }
        Self {
            _root: root,
            remote,
            main_revision,
            feature_revision,
        }
    }

    pub(super) fn move_feature_to(&self, revision: &str) {
        git(
            &self.remote,
            &["update-ref", "refs/heads/feature", revision],
        );
    }
}

pub(super) fn hosting_config(storage_root: PathBuf) -> ProductionHostingConfig {
    ProductionHostingConfig {
        agent_context: None,
        memory_storage: None,
        operator_diagnostic_output: None,
        workspace_storage: None,
        storage_root,
        copilot_executable: PathBuf::from("/usr/bin/false"),
        codex_executable: PathBuf::from("/usr/bin/false"),
        claude_executable: "/usr/bin/false".to_owned(),
        claude_prefix_arguments: Vec::new(),
        claude_process_environment: ClaudeProcessEnvironment::default(),
        pi_executable: "/usr/bin/false".to_owned(),
        pi_prefix_arguments: Vec::new(),
        pi_process_environment: PiProcessEnvironment::default(),
        executable_search_path: "/usr/bin:/bin".to_owned(),
        git_program: PathBuf::from("/usr/bin/git"),
        gh_program: PathBuf::from("/usr/bin/false"),
        process_pool: test_process_pool(),
    }
}

pub(in crate::native_v2_hosting) fn capsule_config(
    storage_root: PathBuf,
) -> ProductionHostingConfig {
    let mut config = hosting_config(storage_root);
    config.process_pool = allocator_process_pool();
    config
}

pub(in crate::native_v2_hosting) fn capsule_allocator(
    config: ProductionHostingConfig,
) -> Result<ProductionCapsuleAllocator, ProductionHostingError> {
    let diagnostics = Arc::new(OperatorDiagnosticStore::new(
        config.operator_diagnostic_output.clone(),
    ));
    ProductionCapsuleAllocator::new(Arc::new(config), diagnostics)
}

pub(super) fn submission(runtime: RuntimePlan, revision: &str, key: &str) -> RunSubmission {
    RunSubmission {
        environment: None,
        title: RunTitle::new("Hosting test run").assert_value_with("title"),
        graph: graph(),
        initial_input: Value::Null,
        runtime,
        source: Some(ResolvedSource {
            remote: None,
            repository: SourceRepositoryId::new("acme/project").assert_value_with("repository"),
            branch: SourceBranchId::new("main").assert_value_with("branch"),
            revision: SourceRevisionId::new(revision).assert_value_with("revision"),
        }),
        submission_key: IdempotencyKey::new(key).assert_value_with("submission key"),
    }
}

pub(super) fn runtime(environment: BTreeSet<EnvironmentVariableName>) -> RuntimePlan {
    let connections = if environment.is_empty() {
        DeclaredConnections::empty()
    } else {
        DeclaredConnections::single(
            "provider",
            DeclaredEnvironment::new(environment).assert_value_with("declared environment"),
        )
        .assert_value_with("declared connection")
    };
    crate::native_v2_candidate::test_support::codex_runtime(
        RunSize::Small,
        BTreeMap::from([(
            NodeName::new("work").assert_value_with("node"),
            NodeRuntimeBinding::Agent {
                lane: None,
                model: worker_catalog::ModelId::new("gpt-5.6").assert_value_with("model"),
                effort: None,
                context_window: None,
                max_tokens: None,
                session_scope: SessionScope::Execution,
                connections,
            },
        )]),
    )
}

pub(super) fn claude_runtime() -> RuntimePlan {
    RuntimePlan::Claude {
        active_memory: Default::default(),
        global_agents: Default::default(),
        provider: ClaudeProvider::Anthropic,
        size: RunSize::Small,
        nodes: BTreeMap::from([(
            NodeName::new("work").assert_value_with("node"),
            NodeRuntimeBinding::Agent {
                lane: None,
                model: worker_catalog::ModelId::new("claude-sonnet-5").assert_value_with("model"),
                effort: Some(ReasoningEffort::Max),
                context_window: None,
                max_tokens: None,
                session_scope: SessionScope::Execution,
                connections: DeclaredConnections::empty(),
            },
        )]),
    }
}

fn graph() -> GraphSpec {
    full_graph(vec![
        json!({
            "kind":"step","name":"work","worker":"agent.work@1",
            "instructions":"Exercise the hosted node.",
            "input":{"kind":"null"},"output":{"kind":"null"},
            "inputBindings":[],"writeBindings":[],"timeoutMs":1000,"attempts":1
        }),
        success_node(),
    ])
}

pub(super) fn portable_filesystem(
    workspace: &Path,
    runtime_home: &Path,
    _process_pool: HostedProcessPool,
) -> Result<CapsuleFilesystem, CapsuleAllocationUnavailable> {
    writable_directory(workspace);
    writable_directory(runtime_home);
    Ok(CapsuleFilesystem {
        workspace: fs::canonicalize(workspace)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?,
        runtime_home: fs::canonicalize(runtime_home)
            .map_err(|_| CapsuleAllocationUnavailable::Runtime)?,
    })
}

pub(super) fn writable_directory(path: &Path) {
    if !path.is_dir() {
        fs::create_dir(path).assert_value_with("create writable directory");
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o777))
        .assert_value_with("writable permissions");
}

pub(super) fn test_process_pool() -> HostedProcessPool {
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };
    if uid == 0 || gid == 0 {
        HostedProcessPool::new(31_002, 31_002, 32_000).assert_value_with("root test pool")
    } else {
        let verifier_base = uid.checked_add(10_000).assert_value_with("test UID range");
        HostedProcessPool::new(uid, gid, verifier_base).assert_value_with("current-user test pool")
    }
}

fn allocator_process_pool() -> HostedProcessPool {
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };
    if uid == 0 || gid == 0 {
        test_process_pool()
    } else {
        let source_uid = uid.checked_sub(1).assert_value_with("allocator source UID");
        HostedProcessPool::new(source_uid, gid, uid)
            .assert_value_with("current-user allocator pool")
    }
}

fn commit(repository: &Path, message: &str) {
    git(
        repository,
        &[
            "-c",
            "user.name=Hosting Test",
            "-c",
            "user.email=hosting@example.invalid",
            "commit",
            "-m",
            message,
        ],
    );
}

fn git(repository: &Path, arguments: &[&str]) {
    assert!(
        git_command(repository)
            .args(arguments)
            .status()
            .assert_value_with("run Git")
            .success()
    );
}

fn git_output(repository: &Path, arguments: &[&str]) -> String {
    let output = git_command(repository)
        .args(arguments)
        .output()
        .assert_value_with("run Git");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .assert_value_with("Git UTF-8")
        .trim()
        .to_owned()
}

fn git_command(repository: &Path) -> Command {
    let mut command = Command::new("/usr/bin/git");
    command
        .arg("-c")
        .arg(format!("safe.directory={}", repository.display()))
        .current_dir(repository)
        .env_clear()
        .env("LANG", "C");
    command
}

fn text(path: &Path) -> &str {
    path.to_str().assert_value_with("UTF-8 path")
}
