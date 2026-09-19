//! Root-supervisor delivery must leave the candidate usable by its contained writer.

use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};

use openengine_cluster_protocol::{FieldName, WorkerOutcome};
use openengine_cluster_testkit::assertions::AssertValue;
use tokio::sync::watch;

use super::*;
use crate::execution::WorkspaceAccessMode;
use crate::execution::driver::{DriverCancellation, WorkspaceCapability};
use crate::execution::process::{
    HostedProcessIdentity, HostedProcessPool, HostedProcessScope, ProcessSessionCommand,
};
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_capsule::{CapsuleFilesystemSpec, prepare_capsule_filesystem};
use crate::native_v2_delivery::DeliveryTarget;
use crate::native_v2_delivery::git::SystemGit;

struct OwnershipFixture {
    directory: TestDirectory,
    workspace: PathBuf,
    runtime: PathBuf,
    pool: HostedProcessPool,
    authority: GhCliDeliveryAuthority,
    source: String,
    target: String,
}

impl OwnershipFixture {
    async fn new() -> Self {
        let directory = TestDirectory::new("delivery-ownership");
        let runtime = directory.child("runtime");
        let workspace = directory.child("workspace");
        let remote = directory.child("remote.git");
        let upstream = directory.child("upstream");
        let pool = HostedProcessPool::new(111_002, 111_002, 112_000, 112_000).assert_value();
        for candidate in [&workspace, &remote, &upstream] {
            prepare_capsule_filesystem(CapsuleFilesystemSpec {
                workspace: candidate,
                runtime_home: &runtime,
                process_pool: pool,
            })
            .assert_value();
        }
        let outside = directory.write("outside", "root-owned sentinel\n");
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o600)).assert_value();
        let writer = pool
            .identity(HostedProcessScope::WriterExecution(1))
            .assert_value();
        let environment = BTreeMap::from([
            ("REMOTE".to_owned(), remote.display().to_string()),
            ("UPSTREAM".to_owned(), upstream.display().to_string()),
            ("OUTSIDE".to_owned(), outside.display().to_string()),
        ]);
        run_provider(
            writer,
            ProviderProbe {
                workspace: &workspace,
                runtime: &runtime,
                environment,
            },
            r#"set -eu
umask 022
git init --quiet --initial-branch=main
git config user.name Fixture
git config user.email fixture@example.invalid
printf 'base\n' > conflict.txt
ln -s "$OUTSIDE" external-link
git add --all
git commit --quiet -m baseline
git clone --quiet --bare . "$REMOTE"
git clone --quiet "$REMOTE" "$UPSTREAM"
git -C "$UPSTREAM" config user.name Fixture
git -C "$UPSTREAM" config user.email fixture@example.invalid
printf 'upstream\n' > "$UPSTREAM/conflict.txt"
mkdir -p "$UPSTREAM/new-upstream/nested"
printf 'preserve upstream\n' > "$UPSTREAM/new-upstream/nested/original.txt"
git -C "$UPSTREAM" add --all
git -C "$UPSTREAM" commit --quiet -m upstream
git -C "$UPSTREAM" push --quiet origin main
printf 'candidate\n' > conflict.txt
"#,
        )
        .await;
        let source = read_revision(&workspace.join(".git/refs/heads/main"));
        let target = read_revision(&upstream.join(".git/refs/heads/main"));
        let git_program = git_wrapper(&directory, &remote, writer);
        let reference =
            serde_json::json!({"ref":"refs/heads/main","object":{"sha":target,"type":"commit"}});
        let gh_program = directory.write_executable(
            "gh",
            &format!(
                r#"#!/bin/sh
case "$*" in
  *repos/acme/project/pulls*) printf '[]\n' ;;
  *git/ref/heads/zeroshot/*) printf 'gh: Not Found (HTTP 404)\n' >&2; exit 1 ;;
  *) printf '%s\n' '{reference}' ;;
esac
"#
            ),
        );
        let mut config = GhCliAuthorityConfig::hosted(runtime.clone());
        config.git_program = git_program;
        config.gh_program = gh_program;
        config.api_deadline = Duration::from_secs(20);
        config.push_deadline = config.api_deadline;
        config.git_identity = Some(pool.identity(HostedProcessScope::Writer).assert_value());
        Self {
            directory,
            workspace,
            runtime,
            pool,
            authority: GhCliDeliveryAuthority::new(config),
            source,
            target,
        }
    }

    fn delivery_target(&self) -> DeliveryTarget {
        DeliveryTarget::new("acme/project", "main", &self.source).assert_value()
    }

    fn system_git(&self) -> SystemGit {
        SystemGit::new(self.authority.config.git_program.clone())
            .with_identity(self.authority.config.git_identity)
    }

    async fn repair_and_redeliver(&self) {
        assert_eq!(
            read_revision(&self.workspace.join(".git/MERGE_HEAD")),
            self.target
        );
        assert!(
            fs::read_to_string(self.workspace.join("conflict.txt"))
                .assert_value()
                .contains("<<<<<<<")
        );
        assert_candidate_owner(
            &self.workspace,
            self.pool
                .identity(HostedProcessScope::Writer)
                .assert_value(),
        );
        let repair = self
            .pool
            .identity(HostedProcessScope::WriterExecution(2))
            .assert_value();
        run_provider(
            repair,
            ProviderProbe {
                workspace: &self.workspace,
                runtime: &self.runtime,
                environment: BTreeMap::new(),
            },
            r#"set -eu
test -z "${GH_TOKEN-}"
test -z "${GIT_CONFIG_VALUE_1-}"
printf 'candidate + upstream\n' > conflict.txt
printf '+ repair\n' >> new-upstream/nested/original.txt
printf 'new repair file\n' > new-upstream/nested/repair.txt
test "$(cat new-upstream/nested/original.txt)" = 'preserve upstream
+ repair'
! cat external-link 2>/dev/null
! printf forbidden > external-link 2>/dev/null
git add --all
git commit --quiet -m 'repair preserves upstream'
printf 'later worker change\n' > after-repair.txt
"#,
        )
        .await;
        let revision = self
            .system_git()
            .prepare_revision(&self.workspace, &self.source, "feat: trusted follow-up")
            .await
            .assert_value();
        let probe = self
            .pool
            .identity(HostedProcessScope::WriterExecution(3))
            .assert_value();
        run_provider(
            probe,
            ProviderProbe {
                workspace: &self.workspace,
                runtime: &self.runtime,
                environment: BTreeMap::from([
                    ("EXPECTED_TARGET".to_owned(), self.target.clone()),
                    ("EXPECTED_HEAD".to_owned(), revision),
                ]),
            },
            r#"set -eu
git merge-base --is-ancestor "$EXPECTED_TARGET" HEAD
test "$(git rev-parse HEAD)" = "$EXPECTED_HEAD"
test -z "$(git status --porcelain=v1)"
test "$(git log -1 --format=%s)" = 'feat: trusted follow-up'
test "$(git config user.name)" = Zeroshot
test "$(git config user.email)" = delivery@zeroshot.invalid
"#,
        )
        .await;
        assert_candidate_owner(&self.workspace, repair);
        let outside = self.directory.child("outside");
        let metadata = fs::metadata(&outside).assert_value();
        assert_eq!(
            (metadata.uid(), metadata.gid(), metadata.mode() & 0o777),
            (0, 0, 0o600)
        );
        assert_eq!(
            fs::read_to_string(outside).assert_value(),
            "root-owned sentinel\n"
        );
        let config = fs::read_to_string(self.workspace.join(".git/config")).assert_value();
        assert!(!config.contains("test-token"));
        assert!(!config.contains("extraheader"));
        self.assert_command_identity();
    }

    fn assert_command_identity(&self) {
        let owner = self
            .pool
            .identity(HostedProcessScope::Writer)
            .assert_value();
        let mut fetches = 0;
        for entry in fs::read_dir(self.directory.child("commands")).assert_value() {
            let capture = fs::read_to_string(entry.assert_value().path()).assert_value();
            assert!(
                capture.starts_with(&format!("uid={} gid={} groups=", owner.uid(), owner.gid())),
                "{capture}"
            );
            let identity = capture.lines().next().assert_value();
            let groups = identity
                .split("groups=")
                .nth(1)
                .assert_value()
                .split(" cwd=")
                .next()
                .assert_value();
            assert_eq!(
                groups,
                owner.gid().to_string(),
                "supervisor groups leaked: {identity}"
            );
            assert!(
                identity.contains(" cwd=/ "),
                "trusted Git inherited its launcher cwd: {identity}"
            );
            if capture.contains("arg=fetch\n") {
                fetches += 1;
                assert!(identity.ends_with("token=test-token"));
            } else {
                assert!(identity.ends_with("token=unset"));
            }
        }
        assert!(fetches > 0);
    }
}

#[tokio::test]
async fn root_target_integration_preserves_contained_writer_ownership() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only delivery ownership gate skipped outside capsule identity");
        return;
    }
    let fixture = OwnershipFixture::new().await;
    let target = fixture.delivery_target();
    let result = fixture
        .authority
        .reconcile_delivery_target(
            GitHubTargetReconciliation {
                workspace: &fixture.workspace,
                target: &target,
                commit_message: "feat: candidate",
            },
            GitHubCredential("test-token"),
        )
        .await
        .assert_value();
    assert_eq!(result.target_revision, fixture.target);
    assert!(matches!(
        result.outcome,
        GitHubReconciliationOutcome::NeedsWork(_)
    ));
    fixture.repair_and_redeliver().await;
}

#[tokio::test]
async fn root_published_conflict_preserves_contained_writer_ownership() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only delivery ownership gate skipped outside capsule identity");
        return;
    }
    let fixture = OwnershipFixture::new().await;
    let candidate = fixture
        .system_git()
        .prepare_revision(&fixture.workspace, &fixture.source, "feat: candidate")
        .await
        .assert_value();
    let request = GitHubConflictRequest {
        workspace: fixture.workspace.clone(),
        review: GitHubReviewReceipt {
            review_id: "17".to_owned(),
            repository: "acme/project".to_owned(),
            target_branch: "main".to_owned(),
            head_branch: "zeroshot/ownership-test".to_owned(),
            head_revision: candidate,
        },
    };
    let result = fixture
        .authority
        .materialize_merge_conflict(&request, GitHubCredential("test-token"))
        .await
        .assert_value();
    assert!(matches!(result, GitHubConflictOutcome::Materialized(_)));
    fixture.repair_and_redeliver().await;
}

#[derive(Clone, Copy)]
enum EscapedExit {
    Repair,
    Failure,
    Cancel,
}

#[tokio::test]
async fn root_delivery_reaps_detached_credential_helpers_before_writer_repair() {
    exercise_escaped_helper(EscapedExit::Repair).await;
}

#[tokio::test]
async fn root_failed_delivery_reaps_detached_credential_helpers_before_handoff() {
    exercise_escaped_helper(EscapedExit::Failure).await;
}

#[tokio::test]
async fn root_cancelled_delivery_reaps_detached_credential_helpers_before_handoff() {
    exercise_escaped_helper(EscapedExit::Cancel).await;
}

async fn exercise_escaped_helper(exit: EscapedExit) {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("root-only delivery cleanup gate skipped outside capsule identity");
        return;
    }
    let fixture = OwnershipFixture::new().await;
    let marker = install_escaped_helper(&fixture, exit);
    let mut handle = start_delivery(&fixture).await;
    let mut durable = handle.take_initial_output().assert_value();
    let drain = tokio::spawn(async move { while durable.recv().await.is_ok() {} });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .assert_value_with("authenticated fetch must launch the escaped helper");
    let pid = read_revision(&marker).parse().assert_value();
    assert!(pid > 0);
    let escaped = EscapedHelper(pid);
    if matches!(exit, EscapedExit::Cancel) {
        handle.cancel();
    }
    let completion = tokio::time::timeout(Duration::from_secs(15), handle.completion())
        .await
        .assert_value_with("delivery handoff must be bounded");
    drain.await.assert_value();
    if matches!(exit, EscapedExit::Cancel) {
        assert!(matches!(
            completion,
            Err(crate::native_v2_runner::NodeRunnerError::Cancelled)
        ));
    } else {
        let outcome = completion.assert_value().outcome;
        let WorkerOutcome::Verifier { signals, .. } = outcome else {
            panic!("delivery must return repair feedback: {outcome:?}");
        };
        assert_eq!(
            signals
                .get(&FieldName::new("delivery").assert_value())
                .assert_value()
                .as_str(),
            "repair_required"
        );
    }
    escaped.assert_reaped();
    fs::remove_file(marker).assert_value();
    if matches!(exit, EscapedExit::Repair) {
        fixture.repair_and_redeliver().await;
    } else {
        let writer = fixture
            .pool
            .identity(HostedProcessScope::WriterExecution(2))
            .assert_value();
        run_provider(
            writer,
            ProviderProbe {
                workspace: &fixture.workspace,
                runtime: &fixture.runtime,
                environment: BTreeMap::new(),
            },
            r#"set -eu
test -z "${GH_TOKEN-}"
printf 'work after interrupted delivery\n' > after-interrupted-delivery.txt
git add --all
git commit --quiet -m 'continue after interrupted delivery'
"#,
        )
        .await;
        assert_candidate_owner(&fixture.workspace, writer);
        fixture.assert_command_identity();
    }
}

fn install_escaped_helper(fixture: &OwnershipFixture, exit: EscapedExit) -> PathBuf {
    let marker = fixture.directory.child("commands/escaped-pid");
    let wrapper = fs::read_to_string(&fixture.authority.config.git_program).assert_value();
    let ending = match exit {
        EscapedExit::Repair => "",
        EscapedExit::Failure => "printf 'injected fetch failure\\n' >&2; exit 17",
        EscapedExit::Cancel => "exec /usr/bin/sleep 60",
    };
    let replacement = format!(
        r#"for argument in "$@"; do
  if [[ "$argument" == fetch ]]; then
    /usr/bin/setsid /bin/sh -c 'printf "%s\n" "$$" > "$1"; exec /usr/bin/sleep 60' \
      credential-helper '{}' </dev/null >/dev/null 2>&1 &
    for attempt in {{1..100}}; do
      test -s '{}' && break
      /usr/bin/sleep 0.01
    done
    test -s '{}'
    {ending}
  fi
done
exec /usr/bin/git "${{arguments[@]}}""#,
        marker.display(),
        marker.display(),
        marker.display(),
    );
    fs::write(
        &fixture.authority.config.git_program,
        wrapper.replace("exec /usr/bin/git \"${arguments[@]}\"", &replacement),
    )
    .assert_value();
    marker
}

async fn start_delivery(fixture: &OwnershipFixture) -> crate::native_v2_runner::NodeHandle {
    use crate::native_v2_candidate::test_support::{admit, full_graph, git_delivery_node, success_node};
    use crate::native_v2_contract as contract;
    use crate::native_v2_delivery::{
        DeliveryPollPolicy, NativeV2DeliveryAdapter, NativeV2DeliveryConfig,
    };
    use crate::native_v2_runner::{NativeNodeRunner, NodeRunRequest, NodeRunner, ResolvedEnvironment};
    use openengine_cluster_protocol::{IdempotencyKey, NodeName, RunId, WorkerRef};
    use std::sync::Arc;

    let token_name = contract::EnvironmentVariableName::new("GH_TOKEN").assert_value();
    let binding = contract::NodeRuntimeBinding::GitDelivery {
        connections: contract::DeclaredConnections::single(
            "github",
            contract::DeclaredEnvironment::new([token_name.clone()]).assert_value(),
        )
        .assert_value(),
        pull_request_feedback: Default::default(),
    };
    let node = NodeName::new("deliver").assert_value();
    let admitted = admit(contract::RunSubmission {
        title: contract::RunTitle::new("Ownership handoff").assert_value(),
        graph: full_graph(vec![git_delivery_node(), success_node()]),
        initial_input: serde_json::Value::Null,
        runtime: contract::RuntimePlan::Codex {
            provider: contract::CodexProvider::OpenAi,
            size: contract::RunSize::Small,
            nodes: BTreeMap::from([(node.clone(), binding.clone())]),
        },
        source: contract::ResolvedSource {
            repository: contract::SourceRepositoryId::new("acme/project").assert_value(),
            branch: contract::SourceBranchId::new("main").assert_value(),
            revision: contract::SourceRevisionId::new(&fixture.source).assert_value(),
        },
        submission_key: IdempotencyKey::new("ownership-handoff").assert_value(),
    })
    .await;
    let adapter = Arc::new(NativeV2DeliveryAdapter::new(
        NativeV2DeliveryConfig {
            git_identity: fixture.authority.config.git_identity,
            workspace: fixture.workspace.clone(),
            git_program: fixture.authority.config.git_program.clone(),
            target: fixture.delivery_target(),
            poll: DeliveryPollPolicy::new(1, Duration::ZERO).assert_value(),
        },
        Arc::new(fixture.authority.clone()),
    ));
    let runner = NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value();
    let environment = ResolvedEnvironment::exact(
        &binding,
        BTreeMap::from([(token_name, "test-token".to_owned())]),
    )
    .assert_value();
    runner
        .start(NodeRunRequest {
            invocation: contract::NodeInvocation {
                reference: contract::ExecutionRef {
                    run_id: RunId::new("ownership-handoff"),
                    node,
                    node_instance: contract::NodeInstanceId::new(1).assert_value(),
                    execution: contract::ExecutionId::new(1).assert_value(),
                },
                worker: WorkerRef::new(contract::GIT_DELIVERY_MERGE_V2_WORKER_REF).assert_value(),
                instructions: None,
                input: serde_json::Value::Null,
                binding,
            },
            environment,
        })
        .await
        .assert_value()
}

struct EscapedHelper(i32);

impl EscapedHelper {
    fn assert_reaped(&self) {
        let environment = fs::read(format!("/proc/{}/environ", self.0)).unwrap_or_default();
        assert!(
            !environment
                .split(|byte| *byte == 0)
                .any(|entry| entry == b"GH_TOKEN=test-token"),
            "trusted Git left a detached process holding its credential at the writer UID"
        );
        let live = fs::read_to_string(format!("/proc/{}/stat", self.0)).is_ok_and(|stat| {
            stat.rsplit_once(") ")
                .is_some_and(|(_, fields)| !matches!(fields.as_bytes().first(), Some(b'Z' | b'X')))
        });
        assert!(
            !live,
            "trusted Git detached process survived the delivery handoff"
        );
    }
}

impl Drop for EscapedHelper {
    fn drop(&mut self) {
        // On a failing regression, also stop and reap the deliberately escaped test helper.
        unsafe {
            libc::kill(self.0, libc::SIGKILL);
            while libc::waitpid(self.0, std::ptr::null_mut(), 0) == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR)
            {}
        }
    }
}

struct ProviderProbe<'a> {
    workspace: &'a Path,
    runtime: &'a Path,
    environment: BTreeMap<String, String>,
}

async fn run_provider(identity: HostedProcessIdentity, probe: ProviderProbe<'_>, script: &str) {
    let ProviderProbe {
        workspace,
        runtime,
        mut environment,
    } = probe;
    let home = identity.prepare_private_home(runtime).assert_value();
    environment.insert("HOME".to_owned(), home.display().to_string());
    environment.insert("PATH".to_owned(), "/usr/bin:/bin".to_owned());
    let (cancellation, receiver) = watch::channel(false);
    let command = ProcessSessionCommand {
        program: "/bin/sh".to_owned(),
        argv: vec!["-c".to_owned(), script.to_owned()],
        environment,
        workspace: WorkspaceCapability {
            current_dir: workspace.to_owned(),
            mode: WorkspaceAccessMode::ReadWrite,
        },
        deadline: Some(tokio::time::Instant::now() + Duration::from_secs(20)),
    };
    let mut process = identity
        .runner()
        .open(command, DriverCancellation::new(receiver))
        .await
        .assert_value();
    process.close_stdin().await.assert_value();
    while process.recv_stdout().await.is_some() {}
    let output = process.wait().await.assert_value();
    assert_eq!(
        output.exit_code,
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr_tail)
    );
    assert!(output.cleanup.proves_tree_empty());
    drop(cancellation);
}

fn git_wrapper(directory: &TestDirectory, remote: &Path, writer: HostedProcessIdentity) -> PathBuf {
    let capture = directory.child("commands");
    fs::create_dir(&capture).assert_value();
    std::os::unix::fs::chown(&capture, Some(writer.uid()), Some(writer.gid())).assert_value();
    directory.write_executable(
        "git",
        &format!(
            r#"#!/bin/bash
set -eu
{{
  /usr/bin/printf 'uid=%s gid=%s groups=%s cwd=%s token=%s\n' \
    "$(/usr/bin/id -u)" "$(/usr/bin/id -g)" "$(/usr/bin/id -G)" "$PWD" "${{GH_TOKEN-unset}}"
  /usr/bin/printf 'arg=%s\n' "$@"
}} > '{capture}/'"$$"
arguments=()
for argument in "$@"; do
  if [[ "$argument" == 'https://github.com/acme/project.git' ]]; then
    arguments+=('{remote}')
  else
    arguments+=("$argument")
  fi
done
exec /usr/bin/git "${{arguments[@]}}"
"#,
            capture = capture.display(),
            remote = remote.display()
        ),
    )
}

fn read_revision(path: &Path) -> String {
    fs::read_to_string(path).assert_value().trim().to_owned()
}

fn assert_candidate_owner(path: &Path, identity: HostedProcessIdentity) {
    let metadata = fs::symlink_metadata(path).assert_value();
    assert_eq!(
        (metadata.uid(), metadata.gid()),
        (identity.uid(), identity.gid()),
        "wrong owner: {}",
        path.display()
    );
    if metadata.is_dir() {
        for entry in fs::read_dir(path).assert_value() {
            assert_candidate_owner(&entry.assert_value().path(), identity);
        }
    }
}
