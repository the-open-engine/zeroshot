use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::{SnapshotId, capture, metadata, restore, restore_staged};

struct Fixture {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    snapshots: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().assert_value();
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).assert_value();
        let snapshots = root.path().join("snapshots");
        Self {
            _root: root,
            workspace,
            snapshots,
        }
    }

    fn initialize_git(&self) {
        initialize_git(&self.workspace);
    }

    fn tree(&self, id: &SnapshotId) -> PathBuf {
        self.snapshots.join(id.as_str()).join("workspace")
    }
}

fn initialize_git(directory: &Path) {
    fs::create_dir_all(directory).assert_value();
    git(directory, &["init", "-b", "main"]);
    fs::write(directory.join("tracked"), "initial").assert_value();
    git(directory, &["add", "tracked"]);
    git(directory, &["commit", "-m", "initial"]);
}

fn add_submodule(directory: &Path, source: &Path, name: &str) {
    git(
        directory,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            source.to_str().assert_value(),
            name,
        ],
    );
    git(directory, &["commit", "-am", "add module"]);
}

fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args([
            "-c",
            "user.name=Checkpoint tests",
            "-c",
            "user.email=checkpoint@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(arguments)
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .assert_value()
        .trim()
        .to_owned()
}

#[test]
fn staging_directory_is_private_at_creation_and_removed_with_its_contents() {
    let root = tempfile::tempdir().assert_value();
    let stage = super::private_stage(root.path(), ".test-").assert_value();
    let path = stage.path().to_owned();
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use crate::execution::platform;
        let directory = platform::open_directory(&path).assert_value();
        platform::windows::security::validate(directory.as_raw_handle()).assert_value();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).assert_value().permissions().mode() & 0o777,
            0o700
        );
    }
    fs::create_dir(path.join("nested")).assert_value();
    fs::write(path.join("nested/file"), "unfinished capture").assert_value();
    drop(stage);
    assert!(!path.exists());
}

#[test]
fn workspace_copy_reads_file_contents_and_preserves_attributes() {
    let root = tempfile::tempdir().assert_value();
    let source = root.path().join("source-λ");
    let destination = root.path().join("copied-λ");
    let bytes = b"nonempty checkpoint contents\0with binary data";
    fs::write(&source, bytes).assert_value();
    let mut permissions = fs::metadata(&source).assert_value().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&source, permissions).assert_value();
    let result =
        crate::native_v2_capsule::provider_process::copy_workspace_entry(&source, &destination);
    assert!(result.is_ok(), "workspace file copy failed: {result:?}");
    assert_eq!(fs::read(&destination).assert_value(), bytes);
    assert!(
        fs::metadata(&destination)
            .assert_value()
            .permissions()
            .readonly()
    );
    assert!(
        fs::metadata(&source)
            .assert_value()
            .permissions()
            .readonly()
    );
}

#[test]
fn readonly_workspace_files_remain_readonly_through_capture_and_restore() {
    let fixture = Fixture::new();
    let file = fixture.workspace.join("readonly");
    fs::write(&file, "checkpoint").assert_value();
    let mut permissions = fs::metadata(&file).assert_value().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&file, permissions).assert_value();
    let result = capture(&fixture.workspace, &fixture.snapshots, &());
    assert!(result.is_ok(), "read-only capture failed: {result:?}");
    let id = result.assert_value();
    assert!(fs::metadata(&file).assert_value().permissions().readonly());
    assert!(
        fs::metadata(fixture.tree(&id).join("readonly"))
            .assert_value()
            .permissions()
            .readonly()
    );

    fs::rename(&file, fixture.workspace.join("old-readonly")).assert_value();
    fs::write(&file, "later edits").assert_value();
    let result = restore(&fixture.snapshots, &id, &fixture.workspace);
    assert!(result.is_ok(), "read-only restore failed: {result:?}");
    assert_eq!(fs::read_to_string(&file).assert_value(), "checkpoint");
    assert!(fs::metadata(&file).assert_value().permissions().readonly());
    assert!(!fixture.workspace.join("old-readonly").exists());
}

#[test]
fn canonical_workspace_captures_and_restores_its_committed_git_head() {
    let fixture = Fixture::new();
    fixture.initialize_git();
    let expected = git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let workspace = fs::canonicalize(&fixture.workspace).assert_value();
    let result = capture(&workspace, &fixture.snapshots, &());
    assert!(result.is_ok(), "canonical Git capture failed: {result:?}");
    let id = result.assert_value();
    assert_eq!(git(&fixture.tree(&id), &["rev-parse", "HEAD"]), expected);
    fs::write(workspace.join("tracked"), "later edits").assert_value();
    let result = restore(&fixture.snapshots, &id, &workspace);
    assert!(result.is_ok(), "canonical Git restore failed: {result:?}");
    assert_eq!(git(&workspace, &["rev-parse", "HEAD"]), expected);
    assert_eq!(
        fs::read_to_string(workspace.join("tracked")).assert_value(),
        "initial"
    );
}

#[test]
fn capture_is_immutable_and_restore_replaces_the_entire_selected_tree() {
    let fixture = Fixture::new();
    fs::write(fixture.workspace.join("source"), "checkpoint").assert_value();
    fs::write(fixture.workspace.join(".gitignore"), "build/\n").assert_value();
    fs::create_dir(fixture.workspace.join("build")).assert_value();
    fs::write(
        fixture.workspace.join("build/generated"),
        "ignored artifact",
    )
    .assert_value();
    let id = capture(
        &fixture.workspace,
        &fixture.snapshots,
        &json!({"node": "writer"}),
    )
    .assert_value();
    fs::write(fixture.workspace.join("source"), "later edits").assert_value();
    fs::remove_dir_all(fixture.workspace.join("build")).assert_value();
    fs::create_dir(fixture.workspace.join("later-directory")).assert_value();
    fs::write(fixture.workspace.join("later-directory/file"), "later").assert_value();

    assert_eq!(
        fs::read_to_string(fixture.tree(&id).join("source")).assert_value(),
        "checkpoint"
    );
    restore(&fixture.snapshots, &id, &fixture.workspace).assert_value();
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("source")).assert_value(),
        "checkpoint"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("build/generated")).assert_value(),
        "ignored artifact"
    );
    assert!(!fixture.workspace.join("later-directory").exists());
    assert_eq!(
        metadata::<serde_json::Value>(&fixture.snapshots, &id).assert_value(),
        json!({"node": "writer"})
    );
    fs::write(fixture.workspace.join("source"), "resumed edits").assert_value();
    assert_eq!(
        fs::read_to_string(fixture.tree(&id).join("source")).assert_value(),
        "checkpoint"
    );
}

#[test]
fn disposable_restic_stage_installs_without_a_second_workspace_copy() {
    let fixture = Fixture::new();
    fs::write(fixture.workspace.join("source"), "checkpoint").assert_value();
    let id = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    let stage = fixture.snapshots.join(id.as_str());
    fs::write(fixture.workspace.join("source"), "later edits").assert_value();
    fs::write(fixture.workspace.join("extra"), "remove me").assert_value();

    restore_staged(&stage, &fixture.workspace).assert_value();

    assert_eq!(
        fs::read_to_string(fixture.workspace.join("source")).assert_value(),
        "checkpoint"
    );
    assert!(!fixture.workspace.join("extra").exists());
    assert_eq!(
        fs::read_dir(stage.join("workspace")).assert_value().count(),
        0
    );
}

#[cfg(unix)]
#[test]
fn preserves_executables_symlinks_times_and_independent_inodes_under_private_root() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    let fixture = Fixture::new();
    let program = fixture.workspace.join("program");
    fs::write(&program, "#!/bin/sh\nexit 0\n").assert_value();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o751)).assert_value();
    let outside = fixture._root.path().join("external");
    fs::create_dir(&outside).assert_value();
    fs::write(outside.join("untouched"), "external").assert_value();
    symlink(&outside, fixture.workspace.join("external-link")).assert_value();
    let before = fs::metadata(&program).assert_value();
    let id = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    let copied = fs::metadata(fixture.tree(&id).join("program")).assert_value();
    assert_ne!(before.ino(), copied.ino());
    assert_eq!(before.permissions().mode(), copied.permissions().mode());
    assert_eq!(
        (before.mtime(), before.mtime_nsec()),
        (copied.mtime(), copied.mtime_nsec())
    );
    let snapshot_root = fs::metadata(fixture.snapshots.join(id.as_str())).assert_value();
    assert_eq!(snapshot_root.permissions().mode() & 0o777, 0o700);
    assert_eq!(
        fs::read_link(fixture.tree(&id).join("external-link")).assert_value(),
        outside
    );
    fs::remove_file(&program).assert_value();
    fs::remove_file(fixture.workspace.join("external-link")).assert_value();
    fs::create_dir(fixture.workspace.join("external-link")).assert_value();
    fs::write(fixture.workspace.join("external-link/after"), "later").assert_value();
    restore(&fixture.snapshots, &id, &fixture.workspace).assert_value();
    assert_eq!(
        fs::metadata(&program).assert_value().permissions().mode() & 0o777,
        0o751
    );
    assert_eq!(
        fs::read_to_string(outside.join("untouched")).assert_value(),
        "external"
    );
}

#[test]
fn rejects_overlapping_roots_and_noncanonical_snapshot_identities() {
    let fixture = Fixture::new();
    let nested = fixture.workspace.join("snapshots");
    assert!(capture(&fixture.workspace, &nested, &()).is_err());
    assert!(!nested.exists());
    let id = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    assert!(
        restore(
            &fixture.snapshots,
            &id,
            &fixture.snapshots.join("workspace")
        )
        .is_err()
    );
    for value in [
        "../workspace",
        ".",
        "019F7E80-0000-7000-8000-000000000000",
        "not-a-uuid",
    ] {
        assert!(serde_json::from_value::<SnapshotId>(json!(value)).is_err());
    }
}

#[test]
fn metadata_failure_never_publishes_a_partial_snapshot() {
    let fixture = Fixture::new();
    fs::write(fixture.workspace.join("kept"), "old snapshot").assert_value();
    let first = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    let oversized = "x".repeat(super::MAX_METADATA_BYTES as usize + 1);
    assert!(capture(&fixture.workspace, &fixture.snapshots, &oversized).is_err());
    let entries = fs::read_dir(&fixture.snapshots)
        .assert_value()
        .map(|entry| entry.assert_value().file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, vec![std::ffi::OsString::from(first.as_str())]);
}

#[test]
fn linked_worktree_restore_preserves_shared_refs_and_configuration() {
    let fixture = Fixture::new();
    fixture.initialize_git();
    let linked = fixture._root.path().join("linked");
    git(
        &fixture.workspace,
        &[
            "worktree",
            "add",
            "-b",
            "writer",
            linked.to_str().assert_value(),
        ],
    );
    git(&linked, &["sparse-checkout", "set", "--no-cone", "tracked"]);
    let sparse = linked.join(git(
        &linked,
        &["rev-parse", "--git-path", "info/sparse-checkout"],
    ));
    let sparse_patterns = fs::read(&sparse).assert_value();
    fs::write(linked.join("tracked"), "staged snapshot").assert_value();
    git(&linked, &["add", "tracked"]);
    let staged_blob = git(&linked, &["rev-parse", ":tracked"]);
    fs::write(linked.join("tracked"), "unstaged snapshot").assert_value();
    let head = git(&linked, &["rev-parse", "HEAD"]);
    let original_marker = fs::read(linked.join(".git")).assert_value();
    let id = capture(&linked, &fixture.snapshots, &()).assert_value();

    git(
        &linked,
        &["config", "--worktree", "core.sparseCheckout", "false"],
    );
    fs::write(&sparse, "later patterns").assert_value();
    fs::write(linked.join("tracked"), "subsequent commit").assert_value();
    git(&linked, &["add", "tracked"]);
    git(&linked, &["commit", "-m", "later"]);
    let branch = git(&linked, &["rev-parse", "refs/heads/writer"]);
    git(
        &fixture.workspace,
        &["config", "checkpoint.shared", "preserve"],
    );
    let object = fixture
        .workspace
        .join(".git/objects")
        .join(&staged_blob[..2])
        .join(&staged_blob[2..]);
    fs::remove_file(object).assert_value();
    restore(&fixture.snapshots, &id, &linked).assert_value();

    assert_eq!(
        fs::read(linked.join(".git")).assert_value(),
        original_marker
    );
    assert_eq!(git(&linked, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&linked, &["rev-parse", "--abbrev-ref", "HEAD"]), "HEAD");
    assert_eq!(git(&linked, &["rev-parse", "refs/heads/writer"]), branch);
    assert_eq!(
        git(&fixture.workspace, &["config", "checkpoint.shared"]),
        "preserve"
    );
    assert_eq!(git(&linked, &["show", ":tracked"]), "staged snapshot");
    assert_eq!(fs::read(&sparse).assert_value(), sparse_patterns);
    assert_eq!(
        git(&linked, &["config", "--worktree", "core.sparseCheckout"]),
        "true"
    );
    assert_eq!(
        fs::read_to_string(linked.join("tracked")).assert_value(),
        "unstaged snapshot"
    );
    let fresh = fixture._root.path().join("fresh");
    restore(&fixture.snapshots, &id, &fresh).assert_value();
    assert!(fresh.join(".git").is_dir());
    assert_eq!(git(&fresh, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&fresh, &["show", ":tracked"]), "staged snapshot");
    git(
        &fresh,
        &[
            "fsck",
            "--connectivity-only",
            "--no-reflogs",
            "--no-dangling",
        ],
    );
}

#[test]
fn main_checkout_restore_does_not_rewind_linked_peers_or_shared_branches() {
    let fixture = Fixture::new();
    fixture.initialize_git();
    let peer = fixture._root.path().join("peer");
    git(
        &fixture.workspace,
        &[
            "worktree",
            "add",
            "-b",
            "peer",
            peer.to_str().assert_value(),
        ],
    );
    let shared_cache = fixture.workspace.join(".git/rr-cache");
    fs::create_dir(&shared_cache).assert_value();
    fs::write(shared_cache.join("shared"), "earlier cache").assert_value();
    let head = git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let id = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    assert!(!fixture.tree(&id).join(".git/worktrees").exists());
    fs::write(shared_cache.join("shared"), "new peer resolution").assert_value();
    fs::write(fixture.workspace.join("tracked"), "main later").assert_value();
    git(&fixture.workspace, &["commit", "-am", "main later"]);
    let main_branch = git(&fixture.workspace, &["rev-parse", "main"]);
    fs::write(peer.join("tracked"), "peer later").assert_value();
    git(&peer, &["commit", "-am", "peer later"]);
    let peer_head = git(&peer, &["rev-parse", "HEAD"]);

    restore(&fixture.snapshots, &id, &fixture.workspace).assert_value();
    assert_eq!(git(&fixture.workspace, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&fixture.workspace, &["rev-parse", "main"]), main_branch);
    assert_eq!(git(&peer, &["rev-parse", "HEAD"]), peer_head);
    assert_eq!(
        fs::read_to_string(peer.join("tracked")).assert_value(),
        "peer later"
    );
    assert!(fixture.workspace.join(".git/worktrees/peer").is_dir());
    assert_eq!(
        fs::read_to_string(shared_cache.join("shared")).assert_value(),
        "new peer resolution"
    );
}

#[test]
fn refuses_external_git_objects() {
    let fixture = Fixture::new();
    fixture.initialize_git();
    let alternates = fixture.workspace.join(".git/objects/info/alternates");
    fs::write(&alternates, "/outside/object-store\n").assert_value();
    assert!(capture(&fixture.workspace, &fixture.snapshots, &()).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn unsupported_fifo_is_rejected_without_opening_or_publishing_it() {
    use std::os::unix::ffi::OsStrExt;

    let fixture = Fixture::new();
    let fifo = fixture.workspace.join("pipe");
    let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).assert_value();
    // SAFETY: path is a live NUL-terminated filename in this test's private directory.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(capture(&fixture.workspace, &fixture.snapshots, &()).is_err());
    assert_eq!(fs::read_dir(&fixture.snapshots).assert_value().count(), 0);
}

#[test]
fn nested_self_contained_repository_restores_inside_a_non_git_workspace() {
    let fixture = Fixture::new();
    let nested = fixture.workspace.join("vendor/library");
    initialize_git(&nested);
    fs::write(nested.join("ignored-artifact"), "artifact").assert_value();
    let id = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    fs::write(nested.join("tracked"), "later").assert_value();
    git(&nested, &["commit", "-am", "later"]);
    restore(&fixture.snapshots, &id, &fixture.workspace).assert_value();
    assert_eq!(git(&nested, &["show", "HEAD:tracked"]), "initial");
    assert_eq!(
        fs::read_to_string(nested.join("tracked")).assert_value(),
        "initial"
    );
    assert_eq!(
        fs::read_to_string(nested.join("ignored-artifact")).assert_value(),
        "artifact"
    );
}

#[test]
fn recursive_submodules_restore_as_self_contained_repositories_with_valid_status() {
    let fixture = Fixture::new();
    fixture.initialize_git();
    let inner_source = fixture._root.path().join("inner-source");
    let module_source = fixture._root.path().join("module-source");
    initialize_git(&inner_source);
    initialize_git(&module_source);
    add_submodule(&module_source, &inner_source, "inner");
    add_submodule(&fixture.workspace, &module_source, "module");
    git(
        &fixture.workspace,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
            "--recursive",
        ],
    );
    let module = fixture.workspace.join("module");
    let inner = module.join("inner");
    assert!(module.join(".git").is_file());
    assert!(inner.join(".git").is_file());
    fs::write(module.join("tracked"), "dirty module").assert_value();
    fs::write(inner.join("tracked"), "dirty inner").assert_value();
    let status = git(&fixture.workspace, &["submodule", "status", "--recursive"]);
    let module_admin = PathBuf::from(git(&module, &["rev-parse", "--absolute-git-dir"]));
    let captured = capture(&fixture.workspace, &fixture.snapshots, &());
    assert!(captured.is_ok(), "submodule capture failed: {captured:?}");
    let id = captured.assert_value();
    assert!(fixture.tree(&id).join("module/.git").is_dir());
    assert!(fixture.tree(&id).join("module/inner/.git").is_dir());

    git(&module, &["commit", "-am", "later module"]);
    let later_admin_head = git(&module, &["rev-parse", "HEAD"]);
    fs::write(inner.join("tracked"), "later inner").assert_value();
    restore(&fixture.snapshots, &id, &fixture.workspace).assert_value();
    assert!(module.join(".git").is_dir());
    assert!(inner.join(".git").is_dir());
    assert_eq!(
        git(&fixture.workspace, &["submodule", "status", "--recursive"]),
        status
    );
    assert_eq!(
        fs::read_to_string(module.join("tracked")).assert_value(),
        "dirty module"
    );
    assert_eq!(
        fs::read_to_string(inner.join("tracked")).assert_value(),
        "dirty inner"
    );
    assert_eq!(git(&module_admin, &["rev-parse", "HEAD"]), later_admin_head);
}

#[test]
fn stale_private_git_locks_are_captured_and_replaced_without_blocking_repair() {
    let fixture = Fixture::new();
    fixture.initialize_git();
    let clean = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    let administrative = fixture.workspace.join(".git");
    for name in [
        "HEAD.lock",
        "index.lock",
        "config.worktree.lock",
        "config.lock",
    ] {
        fs::write(administrative.join(name), name).assert_value();
    }
    let failed = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    fs::write(administrative.join("HEAD.lock"), "new lock").assert_value();
    fs::remove_file(administrative.join("index.lock")).assert_value();
    fs::write(administrative.join("FETCH_HEAD.lock"), "new lock").assert_value();
    restore(&fixture.snapshots, &failed, &fixture.workspace).assert_value();
    assert_eq!(
        fs::read_to_string(administrative.join("HEAD.lock")).assert_value(),
        "HEAD.lock"
    );
    assert_eq!(
        fs::read_to_string(administrative.join("index.lock")).assert_value(),
        "index.lock"
    );
    assert!(!administrative.join("FETCH_HEAD.lock").exists());
    restore(&fixture.snapshots, &clean, &fixture.workspace).assert_value();
    for name in ["HEAD.lock", "index.lock", "config.worktree.lock"] {
        assert!(!administrative.join(name).exists());
    }
    assert_eq!(
        fs::read_to_string(administrative.join("config.lock")).assert_value(),
        "config.lock"
    );
}

#[test]
fn gitfile_cannot_redirect_snapshot_authority_to_an_unrelated_repository() {
    let fixture = Fixture::new();
    fixture.initialize_git();
    let selected = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    fs::rename(
        fixture.workspace.join(".git"),
        fixture._root.path().join("original-admin"),
    )
    .assert_value();
    let unrelated = fixture._root.path().join("unrelated");
    initialize_git(&unrelated);
    let head = fs::read(unrelated.join(".git/HEAD")).assert_value();
    fs::write(
        fixture.workspace.join(".git"),
        format!("gitdir: {}\n", unrelated.join(".git").display()),
    )
    .assert_value();
    assert!(capture(&fixture.workspace, &fixture.snapshots, &()).is_err());
    assert!(restore(&fixture.snapshots, &selected, &fixture.workspace).is_err());
    assert_eq!(fs::read(unrelated.join(".git/HEAD")).assert_value(), head);
}

#[test]
fn nested_repository_with_linked_peers_refuses_restore_before_mutating_any_files() {
    let fixture = Fixture::new();
    let nested = fixture.workspace.join("nested");
    let peer = fixture._root.path().join("external-peer");
    initialize_git(&nested);
    git(
        &nested,
        &[
            "worktree",
            "add",
            "-b",
            "peer",
            peer.to_str().assert_value(),
        ],
    );
    let peer_head = git(&peer, &["rev-parse", "HEAD"]);
    let id = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    fs::write(nested.join("tracked"), "current edits").assert_value();
    fs::write(fixture.workspace.join("later-file"), "keep").assert_value();
    assert!(restore(&fixture.snapshots, &id, &fixture.workspace).is_err());
    assert_eq!(
        fs::read_to_string(nested.join("tracked")).assert_value(),
        "current edits"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("later-file")).assert_value(),
        "keep"
    );
    assert_eq!(git(&peer, &["rev-parse", "HEAD"]), peer_head);
    assert!(nested.join(".git/worktrees/external-peer").is_dir());
}
