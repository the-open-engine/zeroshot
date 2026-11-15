//! Contained delivery Git runs no program that workspace Git configuration or attributes name.
use super::*;
use crate::source_code_provider::{DeliveryTarget, SourceTargetIntegration, SourceTargetReconciliation};

/// Every program the workspace configuration names records its name here when it runs.
struct Trap {
    canaries: PathBuf,
    program: PathBuf,
}

impl Trap {
    fn new(root: &Path) -> Self {
        let canaries = root.join("canaries");
        std::fs::create_dir(&canaries).assert_value();
        let program = root.join("trap");
        let source = format!(
            "#!/bin/sh\n/usr/bin/touch '{}'/\"$1\"\ncat >/dev/null 2>&1\nexit 0\n",
            canaries.display()
        );
        openengine_cluster_testkit::fixture::write_executable(&program, &source, 0o700)
            .assert_value();
        Self { canaries, program }
    }

    fn fired(&self) -> Vec<String> {
        let mut fired = std::fs::read_dir(&self.canaries)
            .assert_value()
            .map(|entry| {
                entry
                    .assert_value()
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        fired.sort();
        fired
    }

    fn command(&self, name: &str) -> String {
        format!("{} {name}", self.program.display())
    }
}

/// Configures programs wherever workspace Git could select one: drivers selected by in-tree and
/// `info/attributes`, program-valued settings, an included file and a URL rewrite that turns the
/// remote into a command.
fn configure_programs(workspace: &Path, remote: &Path, trap: &Trap) {
    let included = workspace.join(".git/included.config");
    std::fs::write(
        &included,
        format!(
            "[filter \"included\"]\n\tclean = {}\n\tsmudge = {}\n",
            trap.command("included-clean"),
            trap.command("included-smudge")
        ),
    )
    .assert_value();
    // `ext::` runs its command line, here split into the trap and its argument.
    let rewrite = format!("url.ext::{} transport.insteadOf", trap.program.display());
    let settings = [
        ("filter.evil.clean", trap.command("filter-clean")),
        ("filter.evil.smudge", trap.command("filter-smudge")),
        ("filter.evil.required", "true".to_owned()),
        ("merge.evil.driver", trap.command("merge-driver")),
        ("diff.evil.command", trap.command("diff-command")),
        ("diff.evil.textconv", trap.command("diff-textconv")),
        ("diff.evil.trustExitCode", "true".to_owned()),
        ("diff.external", trap.command("diff-external")),
        ("diff.trustExitCode", "true".to_owned()),
        ("core.fsmonitor", trap.command("fsmonitor")),
        ("core.sshCommand", trap.command("ssh")),
        ("core.askPass", trap.command("askpass")),
        ("core.pager", trap.command("pager")),
        ("core.editor", trap.command("editor")),
        ("core.alternateRefsCommand", trap.command("alternate-refs")),
        (
            "credential.helper",
            format!("!{}", trap.command("credential")),
        ),
        ("gpg.program", trap.command("gpg")),
        ("commit.gpgSign", "true".to_owned()),
        ("include.path", included.display().to_string()),
        ("protocol.allow", "always".to_owned()),
        (rewrite.as_str(), remote.display().to_string()),
    ];
    for (key, value) in &settings {
        git(workspace, &["config", "--add", key, value]);
    }
    std::fs::write(
        workspace.join(".gitattributes"),
        "* filter=evil merge=evil diff=evil\nincluded* filter=included\n",
    )
    .assert_value();
    std::fs::create_dir_all(workspace.join(".git/info")).assert_value();
    std::fs::write(
        workspace.join(".git/info/attributes"),
        "shared merge=evil diff=evil\n",
    )
    .assert_value();
}

/// The same Git without delivery's confinement, which proves the configuration is effective.
fn unconfined_add(workspace: &Path) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["add", "--all"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn unconfined_git_runs_the_configured_workspace_programs() {
    let repository = Repository::new();
    let trap = Trap::new(repository.root.path());
    configure_programs(&repository.workspace, &repository.remote, &trap);
    std::fs::write(repository.workspace.join("included-file"), "included").assert_value();
    unconfined_add(&repository.workspace);
    let fired = trap.fired();
    for name in ["filter-clean", "included-clean"] {
        assert!(fired.contains(&name.to_owned()), "{fired:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn configured_programs_never_run_while_delivery_stages_commits_merges_and_pushes() {
    let repository = Repository::new();
    let remote_path = repository.remote.to_str().assert_value().to_owned();
    std::fs::write(
        repository.workspace.join("shared"),
        "one\ntwo\nthree\nfour\nfive\n",
    )
    .assert_value();
    git(&repository.workspace, &["add", "--all"]);
    git(&repository.workspace, &["commit", "--message", "shared"]);
    git(&repository.workspace, &["push", &remote_path, "main"]);
    let base = git(&repository.workspace, &["rev-parse", "HEAD"]);

    // Advance the target so integration performs a real three-way content merge of `shared`.
    let peer = repository.root.path().join("peer");
    git(
        repository.root.path(),
        &["clone", &remote_path, peer.to_str().assert_value()],
    );
    std::fs::write(peer.join("shared"), "ONE\ntwo\nthree\nfour\nfive\n").assert_value();
    git(
        &peer,
        &[
            "-c",
            "user.name=Peer",
            "-c",
            "user.email=peer@example.invalid",
            "commit",
            "--all",
            "--message",
            "target",
        ],
    );
    git(&peer, &["push", "origin", "main"]);
    let target_revision = git(&peer, &["rev-parse", "HEAD"]);

    // Workspace changes, then program-valued configuration.
    std::fs::write(
        repository.workspace.join("shared"),
        "one\ntwo\nthree\nfour\nFIVE\n",
    )
    .assert_value();
    std::fs::write(repository.workspace.join("included-file"), "included\n").assert_value();
    let trap = Trap::new(repository.root.path());
    configure_programs(&repository.workspace, &repository.remote, &trap);

    let transport = repository.transport(
        Path::new("git"),
        &repository.workspace,
        repository.remote.as_os_str(),
    );
    let credential = SourceDeliveryCredential::token("");
    let target = DeliveryTarget::new("acme/project", "main", base.clone()).assert_value();
    let integration = super::super::target::reconcile_target(
        &transport,
        SourceTargetReconciliation {
            workspace: &repository.workspace,
            target: &target,
            commit_message: "agent work",
        },
        &target_revision,
        credential,
    )
    .await
    .assert_value();
    assert!(
        matches!(integration, SourceTargetIntegration::Integrated { .. }),
        "{integration:?}"
    );
    let head = git(&repository.workspace, &["rev-parse", "HEAD"]);
    git(
        &repository.workspace,
        &["merge-base", "--is-ancestor", &target_revision, &head],
    );
    transport
        .push("zeroshot/configured", &head, credential)
        .await
        .assert_value();

    assert_eq!(trap.fired(), Vec::<String>::new());
    assert_eq!(
        git(
            &repository.remote,
            &["rev-parse", "refs/heads/zeroshot/configured"]
        ),
        head
    );
    // Built-in drivers handled the content: nothing was filtered and both edits merged.
    assert_eq!(
        git(&repository.workspace, &["show", &format!("{head}:shared")]),
        "ONE\ntwo\nthree\nfour\nFIVE"
    );
    assert_eq!(
        git(
            &repository.workspace,
            &["show", &format!("{head}:included-file")]
        ),
        "included"
    );
    // Only the retained layout, remote and identity remain; attributes stay as declared.
    let names = git(
        &repository.workspace,
        &[
            "config",
            "--local",
            "--no-includes",
            "--list",
            "--name-only",
        ],
    );
    for name in names.lines() {
        assert!(
            crate::native_v2_delivery::command::retained_repository_key(name),
            "{name} survived confinement"
        );
    }
    assert!(repository.workspace.join(".gitattributes").exists());
    assert!(repository.workspace.join(".git/info/attributes").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn configured_transport_programs_never_replace_the_push_transport() {
    let repository = Repository::new();
    let candidate = repository.commit("candidate");
    let trap = Trap::new(repository.root.path());
    configure_programs(&repository.workspace, &repository.remote, &trap);
    // A remote named after the delivery URL would otherwise also select its pack programs.
    let remote_path = repository.remote.to_str().assert_value().to_owned();
    for key in ["receivepack", "uploadpack"] {
        git(
            &repository.workspace,
            &[
                "config",
                &format!("remote.{remote_path}.{key}"),
                &trap.command(key),
            ],
        );
    }
    let transport = repository.transport(
        Path::new("git"),
        &repository.workspace,
        repository.remote.as_os_str(),
    );
    let credential = SourceDeliveryCredential::token("");
    transport
        .push("zeroshot/candidate", &candidate, credential)
        .await
        .assert_value();
    assert_eq!(
        transport
            .read_ref("zeroshot/candidate", credential)
            .await
            .assert_value(),
        Some(candidate)
    );
    assert_eq!(trap.fired(), Vec::<String>::new());
}

#[cfg(unix)]
#[tokio::test]
async fn delivery_git_never_reaches_a_parent_repository_before_initialization() {
    let repository = Repository::new();
    let trap = Trap::new(repository.root.path());
    git(
        &repository.workspace,
        &["config", "filter.parent.clean", &trap.command("parent")],
    );
    std::fs::write(
        repository.workspace.join(".gitattributes"),
        "* filter=parent\n",
    )
    .assert_value();
    let nested = repository.workspace.join("nested");
    std::fs::create_dir(&nested).assert_value();
    std::fs::write(nested.join("file.txt"), "content\n").assert_value();
    let transport = repository.transport(Path::new("git"), &nested, repository.remote.as_os_str());
    let mut command = transport.workspace_command(&nested).await.assert_value();
    assert_eq!(
        git(&repository.workspace, &["config", "filter.parent.clean"]),
        trap.command("parent")
    );
    // Without its own `.git` the workspace has nothing to confine, and the delivery command
    // itself must not discover the parent repository and run its filter.
    command.args(["add", "--all"]);
    let output = command.capture().await.assert_value();
    assert_ne!(output.exit_status, Some(0), "{output}");
    assert_eq!(trap.fired(), Vec::<String>::new());
}

#[tokio::test]
async fn native_local_configuration_is_left_to_the_invoking_user() {
    let repository = Repository::new();
    git(&repository.workspace, &["config", "rerere.enabled", "true"]);
    let configuration = repository.root.path().join("native.gitconfig");
    std::fs::write(&configuration, "").assert_value();
    let transport = repository.native_transport(
        repository.remote.as_os_str(),
        &configuration,
        BTreeMap::new(),
    );
    transport
        .workspace_command(&repository.workspace)
        .await
        .assert_value();
    assert_eq!(
        git(&repository.workspace, &["config", "rerere.enabled"]),
        "true"
    );
}

/// Adds a populated submodule whose own repository configuration and attributes select a
/// filter.
fn configure_submodule_programs(repository: &Repository, trap: &Trap) -> PathBuf {
    let dependency = repository.root.path().join("dependency");
    std::fs::create_dir(&dependency).assert_value();
    git(&dependency, &["init", "--quiet", "--initial-branch=main"]);
    std::fs::write(dependency.join("tracked"), "tracked\n").assert_value();
    git(&dependency, &["add", "--all"]);
    git(
        &dependency,
        &[
            "-c",
            "user.name=Dependency",
            "-c",
            "user.email=dependency@example.invalid",
            "commit",
            "--message",
            "dependency",
        ],
    );
    git(
        &repository.workspace,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--quiet",
            dependency.to_str().assert_value(),
            "dependency",
        ],
    );
    git(
        &repository.workspace,
        &["commit", "--message", "add dependency"],
    );
    let submodule = repository.workspace.join("dependency");
    git(
        &submodule,
        &[
            "config",
            "filter.nested.clean",
            &trap.command("submodule-clean"),
        ],
    );
    std::fs::write(submodule.join(".gitattributes"), "* filter=nested\n").assert_value();
    git(&submodule, &["add", ".gitattributes"]);
    submodule
}

fn stat_dirty(path: &Path) {
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .assert_value();
    file.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .assert_value();
}

#[cfg(unix)]
#[tokio::test]
async fn submodule_configured_programs_never_run_while_delivery_stages_and_commits() {
    let repository = Repository::new();
    let trap = Trap::new(repository.root.path());
    let submodule = configure_submodule_programs(&repository, &trap);

    // Plain Git starts `git status` inside the submodule, which runs its filter.
    stat_dirty(&submodule.join("tracked"));
    unconfined_add(&repository.workspace);
    assert_eq!(trap.fired(), vec!["submodule-clean".to_owned()]);
    std::fs::remove_file(trap.canaries.join("submodule-clean")).assert_value();
    git(&repository.workspace, &["reset", "--quiet"]);

    stat_dirty(&submodule.join("tracked"));
    std::fs::write(repository.workspace.join("work"), "work\n").assert_value();
    let transport = repository.transport(
        Path::new("git"),
        &repository.workspace,
        repository.remote.as_os_str(),
    );
    let base = git(&repository.workspace, &["rev-parse", "HEAD"]);
    let revision = transport
        .workspace_git()
        .prepare_revision(&repository.workspace, &base, "agent work")
        .await
        .assert_value();
    assert_ne!(revision, base);
    assert_eq!(trap.fired(), Vec::<String>::new());
    assert!(
        git(&submodule, &["config", "--local", "--list", "--name-only"])
            .lines()
            .all(crate::native_v2_delivery::command::retained_repository_key)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn confinement_shares_the_callers_deadline_and_stop_request() {
    let repository = Repository::new();
    // Reading the configuration hangs, as an include of a FIFO would.
    let program = repository.root.path().join("hanging-config-git");
    openengine_cluster_testkit::fixture::write_executable(
        &program,
        "#!/bin/sh\ncase \" $* \" in *\" config --local \"*) exec /bin/sleep 30 ;; esac\nexec /usr/bin/git \"$@\"\n",
        0o700,
    )
    .assert_value();
    let transport = repository.transport(
        &program,
        &repository.workspace,
        repository.remote.as_os_str(),
    );
    let started = std::time::Instant::now();
    let failure = transport
        .execute(
            &["rev-parse".into(), "HEAD".into()],
            SourceDeliveryCredential::token(""),
            Duration::from_secs(1),
        )
        .await
        .expect_err("confinement must not outlive the caller's deadline");
    assert!(failure.timed_out, "{failure}");
    assert!(started.elapsed() < Duration::from_secs(10));

    let (_stop, stopping) = tokio::sync::watch::channel(true);
    let failure = transport
        .with_stop_request(Some(stopping))
        .workspace_command(&repository.workspace)
        .await
        .expect_err("a stop request interrupts confinement");
    assert!(failure.to_string().contains("stopped"), "{failure}");
}

#[cfg(unix)]
#[tokio::test]
async fn delivery_commands_run_on_what_confinement_leaves_of_their_deadline() {
    let repository = Repository::new();
    // Each configuration read takes a second, as a slow filesystem would.
    let program = repository.root.path().join("slow-config-git");
    openengine_cluster_testkit::fixture::write_executable(
        &program,
        "#!/bin/sh\ncase \" $* \" in *\" config --local \"*) /bin/sleep 1 ;; esac\nexec /usr/bin/git \"$@\"\n",
        0o700,
    )
    .assert_value();
    let transport = repository.transport(
        &program,
        &repository.workspace,
        repository.remote.as_os_str(),
    );
    let deadline = Duration::from_secs(20);
    let command = transport
        .workspace_command_within(&repository.workspace, deadline)
        .await
        .assert_value();
    assert!(
        command.deadline() <= deadline - Duration::from_secs(1),
        "{:?}",
        command.deadline()
    );

    // A confinement that leaves nothing fails as itself, not as a zero-second Git command.
    let uninitialized = repository.root.path().join("uninitialized");
    std::fs::create_dir(&uninitialized).assert_value();
    let failure = transport
        .workspace_command_within(&uninitialized, Duration::ZERO)
        .await
        .expect_err("an exhausted deadline leaves no time for the command");
    assert!(failure.to_string().contains("whole deadline"), "{failure}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_submodule_path_never_confines_the_repository_behind_it() {
    let repository = Repository::new();
    let trap = Trap::new(repository.root.path());
    let submodule = configure_submodule_programs(&repository, &trap);
    let outside = repository.root.path().join("outside");
    std::fs::create_dir(&outside).assert_value();
    git(&outside, &["init", "--quiet"]);
    git(&outside, &["config", "outside.setting", "kept"]);
    std::fs::remove_dir_all(&submodule).assert_value();
    std::os::unix::fs::symlink(&outside, &submodule).assert_value();
    for fired in trap.fired() {
        std::fs::remove_file(trap.canaries.join(fired)).assert_value();
    }
    let transport = repository.transport(
        Path::new("git"),
        &repository.workspace,
        repository.remote.as_os_str(),
    );
    transport
        .workspace_command(&repository.workspace)
        .await
        .assert_value();
    assert_eq!(git(&outside, &["config", "outside.setting"]), "kept");
    assert_eq!(trap.fired(), Vec::<String>::new());
}

#[cfg(unix)]
#[tokio::test]
async fn worktree_configuration_is_never_read_once_its_extension_is_removed() {
    let repository = Repository::new();
    let trap = Trap::new(repository.root.path());
    git(
        &repository.workspace,
        &["config", "core.repositoryformatversion", "1"],
    );
    git(
        &repository.workspace,
        &["config", "extensions.worktreeConfig", "true"],
    );
    git(
        &repository.workspace,
        &[
            "config",
            "--worktree",
            "filter.worktree.clean",
            &trap.command("worktree-clean"),
        ],
    );
    std::fs::write(
        repository.workspace.join(".gitattributes"),
        "* filter=worktree\n",
    )
    .assert_value();
    std::fs::write(repository.workspace.join("work"), "work\n").assert_value();
    let transport = repository.transport(
        Path::new("git"),
        &repository.workspace,
        repository.remote.as_os_str(),
    );
    let revision = transport
        .workspace_git()
        .prepare_revision(&repository.workspace, &repository.base, "workspace changes")
        .await
        .assert_value();
    assert_eq!(trap.fired(), Vec::<String>::new());
    assert_eq!(
        git(
            &repository.workspace,
            &["show", &format!("{revision}:work")]
        ),
        "work"
    );
    assert!(
        !git(
            &repository.workspace,
            &["config", "--local", "--list", "--name-only"]
        )
        .lines()
        .any(|name| name == "extensions.worktreeconfig")
    );
}

#[tokio::test]
async fn content_settings_survive_confinement_and_on_demand_fetching_does_not() {
    let repository = Repository::new();
    let kept = [
        ("core.autocrlf", "input"),
        ("core.eol", "lf"),
        ("core.safecrlf", "warn"),
        ("core.sparseCheckout", "false"),
        ("core.sparseCheckoutCone", "true"),
    ];
    for (key, value) in kept {
        git(&repository.workspace, &["config", key, value]);
    }
    git(
        &repository.workspace,
        &["config", "extensions.partialClone", "origin"],
    );
    git(
        &repository.workspace,
        &["config", "remote.origin.promisor", "true"],
    );
    let transport = repository.transport(
        Path::new("git"),
        &repository.workspace,
        repository.remote.as_os_str(),
    );
    transport
        .workspace_command(&repository.workspace)
        .await
        .assert_value();
    for (key, value) in kept {
        assert_eq!(git(&repository.workspace, &["config", key]), value, "{key}");
    }
    let names = git(
        &repository.workspace,
        &["config", "--local", "--list", "--name-only"],
    );
    for removed in ["extensions.partialclone", "remote.origin.promisor"] {
        assert!(
            !names.lines().any(|name| name == removed),
            "{removed}: {names}"
        );
    }
}
