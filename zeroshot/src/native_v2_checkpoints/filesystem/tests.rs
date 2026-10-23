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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path.join("nested"), fs::Permissions::from_mode(0o555)).assert_value();
    }
    drop(stage);
    assert!(!path.exists());
}

#[cfg(windows)]
#[test]
fn disposable_tree_removes_readonly_ordinary_nested_directories() {
    let root = tempfile::tempdir().assert_value();
    let stage = super::private_stage(root.path(), ".test-").assert_value();
    let nested = stage.path().join("nested");
    fs::create_dir(&nested).assert_value();
    let file = nested.join("file");
    fs::write(&file, "unfinished capture").assert_value();
    for entry in [&file, &nested] {
        let mut permissions = fs::metadata(entry).assert_value().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(entry, permissions).assert_value();
    }

    super::remove_disposable_tree(stage.path()).assert_value_with("remove ordinary nested tree");
    assert!(!stage.path().exists());
}

#[cfg(unix)]
#[test]
fn disposable_tree_cleanup_does_not_follow_workspace_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = tempfile::tempdir().assert_value();
    let stage = super::private_stage(root.path(), ".test-").assert_value();
    let outside = root.path().join("outside");
    fs::create_dir(&outside).assert_value();
    fs::write(outside.join("retained"), "original").assert_value();
    let nested = stage.path().join("readonly");
    fs::create_dir(&nested).assert_value();
    symlink(&outside, nested.join("link")).assert_value();
    fs::set_permissions(&nested, fs::Permissions::from_mode(0o555)).assert_value();

    super::remove_disposable_tree(stage.path()).assert_value();

    assert!(!stage.path().exists());
    assert_eq!(
        fs::read_to_string(outside.join("retained")).assert_value(),
        "original"
    );
    assert!(super::remove_disposable_tree(&root.path().join("missing")).is_ok());
    assert!(super::remove_disposable_tree(&outside.join("retained")).is_err());
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

#[cfg(target_os = "macos")]
fn run_macos_tool(program: &str, arguments: &[&str], path: &Path) -> String {
    let output = Command::new(program)
        .args(arguments)
        .arg(path)
        .output()
        .assert_value();
    assert!(output.status.success(), "{program} {arguments:?} failed");
    String::from_utf8(output.stdout).assert_value()
}

#[cfg(target_os = "macos")]
fn macos_volume_type(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;

    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).assert_value();
    // SAFETY: statfs writes only into the zeroed buffer, and the path is NUL-terminated.
    let mut volume = unsafe { std::mem::zeroed::<libc::statfs>() };
    assert_eq!(unsafe { libc::statfs(path.as_ptr(), &mut volume) }, 0);
    // SAFETY: the kernel NUL-terminates f_fstypename within its fixed buffer.
    unsafe { std::ffi::CStr::from_ptr(volume.f_fstypename.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(target_os = "macos")]
#[test]
fn macos_workspace_copy_clones_independent_files_on_apfs() {
    use std::os::unix::fs::PermissionsExt;

    let require_clone = std::env::var_os("ZEROSHOT_REQUIRE_APFS_CLONE").is_some();
    let root = tempfile::tempdir().assert_value();
    // Other volume types take the portable copy, which drops the xattr this test observes.
    let volume = macos_volume_type(root.path());
    if volume != "apfs" {
        assert!(
            !require_clone,
            "required clone probe needs apfs, not {volume}"
        );
        eprintln!("APFS clone test skipped on a {volume} temporary volume");
        return;
    }
    let source = root.path().join("source");
    let destination = root.path().join("cloned");
    fs::write(&source, "original").assert_value();
    // Only a clone carries extended attributes; the portable copy drops them.
    run_macos_tool("xattr", &["-w", "dev.zeroshot.test", "cloned"], &source);
    let mut permissions = fs::metadata(&source).assert_value().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&source, permissions).assert_value();

    crate::native_v2_capsule::provider_process::copy_workspace_entry(&source, &destination)
        .assert_value();

    assert_eq!(
        run_macos_tool("xattr", &["-p", "dev.zeroshot.test"], &destination).trim(),
        "cloned"
    );
    assert!(
        fs::metadata(&destination)
            .assert_value()
            .permissions()
            .readonly()
    );
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o600)).assert_value();
    fs::write(&destination, "changed").assert_value();
    assert_eq!(fs::read_to_string(&source).assert_value(), "original");
}

#[cfg(target_os = "macos")]
#[test]
fn macos_workspace_copy_of_locked_file_stays_removable() {
    use std::os::macos::fs::MetadataExt;

    let root = tempfile::tempdir().assert_value();
    let source = root.path().join("locked");
    let stage = root.path().join("stage");
    fs::create_dir(&stage).assert_value();
    fs::write(&source, "locked").assert_value();
    run_macos_tool("chflags", &["uchg"], &source);

    let copied = crate::native_v2_capsule::provider_process::copy_workspace_entry(
        &source,
        &stage.join("locked"),
    );
    run_macos_tool("chflags", &["nouchg"], &source);

    copied.assert_value();
    let metadata = fs::metadata(stage.join("locked")).assert_value();
    assert_eq!(metadata.st_flags(), 0);
    assert_eq!(
        fs::read_to_string(stage.join("locked")).assert_value(),
        "locked"
    );
    super::remove_disposable_tree(&stage).assert_value();
    assert!(!stage.exists());
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
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            stage.join("metadata.json"),
            fs::Permissions::from_mode(0o644),
        )
        .assert_value();
    }
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

    restore(&fixture.snapshots, &id, &fixture.workspace)
        .unwrap_or_else(|error| panic!("checkpoint restore failed: {error:?}"));
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
    use openengine_cluster_testkit::assertions::AssertError;
    use std::os::unix::ffi::OsStrExt;

    let fixture = Fixture::new();
    let fifo = fixture.workspace.join("pipe");
    let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).assert_value();
    // SAFETY: path is a live NUL-terminated filename in this test's private directory.
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    let error = capture(&fixture.workspace, &fixture.snapshots, &()).assert_error();
    assert_eq!(
        error.to_string(),
        "candidate contains an unsupported filesystem entry \"pipe\""
    );
    assert_eq!(fs::read_dir(&fixture.snapshots).assert_value().count(), 0);
}

#[test]
fn nested_self_contained_repository_restores_inside_a_non_git_workspace() {
    let fixture = Fixture::new();
    let nested = fixture.workspace.join("vendor/library");
    initialize_git(&nested);
    fs::write(nested.join("ignored-artifact"), "artifact").assert_value();
    let captured = capture(&fixture.workspace, &fixture.snapshots, &());
    assert!(captured.is_ok(), "nested Git capture failed: {captured:?}");
    let id = captured.assert_value();
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
    let clean = capture(&fixture.workspace, &fixture.snapshots, &());
    assert!(clean.is_ok(), "initial clean capture failed: {clean:?}");
    let clean = clean.assert_value();
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
    let selected = capture(&fixture.workspace, &fixture.snapshots, &())
        .unwrap_or_else(|error| panic!("checkpoint capture failed: {error:?}"));
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

/// Returns the checkout location that precedes an unresolved worktree link and Git's repair.
fn unresolved_worktree_checkout(error: &std::io::Error) -> String {
    let message = error.to_string();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{message}");
    assert!(
        message.ends_with(
            "; run `git worktree repair` with this checkout's path from its main worktree, \
             or remove the stale checkout"
        ),
        "{message}"
    );
    let (checkout, _) = message
        .split_once(": unresolved Git worktree link: ")
        .assert_value_with(&message);
    checkout.to_owned()
}

#[test]
fn unresolved_nested_worktree_links_name_the_relative_checkout() {
    use openengine_cluster_testkit::assertions::AssertError;

    // Moving the main checkout leaves the nested `.git` file naming a missing directory.
    let dangling = Fixture::new();
    let before = dangling._root.path().join("before");
    initialize_git(&before);
    git(
        &before,
        &["worktree", "add", "-b", "nested", "worktrees/nested"],
    );
    fs::remove_dir(&dangling.workspace).assert_value();
    fs::rename(&before, &dangling.workspace).assert_value();
    // Moving a linked worktree leaves its administrative backpointer naming the old path.
    let moved = Fixture::new();
    moved.initialize_git();
    git(
        &moved.workspace,
        &["worktree", "add", "-b", "nested", "worktrees/nested"],
    );
    let worktrees = moved.workspace.join("worktrees");
    fs::rename(worktrees.join("nested"), worktrees.join("moved")).assert_value();

    for (fixture, checkout) in [(dangling, "nested"), (moved, "moved")] {
        let error = capture(&fixture.workspace, &fixture.snapshots, &()).assert_error();
        assert_eq!(
            unresolved_worktree_checkout(&error),
            format!("Git checkout {:?}", Path::new("worktrees").join(checkout))
        );
        assert_eq!(fs::read_dir(&fixture.snapshots).assert_value().count(), 0);
    }
}

#[test]
fn moved_linked_workspace_is_named_and_refused_by_capture_and_restore() {
    use openengine_cluster_testkit::assertions::AssertError;

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
    let id = capture(&linked, &fixture.snapshots, &()).assert_value();
    let moved = fixture._root.path().join("moved");
    fs::rename(&linked, &moved).assert_value();
    fs::write(moved.join("tracked"), "current edits").assert_value();

    for error in [
        capture(&moved, &fixture.snapshots, &()).assert_error(),
        restore(&fixture.snapshots, &id, &moved).assert_error(),
    ] {
        let checkout = unresolved_worktree_checkout(&error);
        assert!(
            checkout.starts_with("Git checkout \"") && checkout.ends_with("moved\""),
            "{checkout}"
        );
    }
    assert_eq!(fs::read_dir(&fixture.snapshots).assert_value().count(), 1);
    assert_eq!(
        fs::read_to_string(moved.join("tracked")).assert_value(),
        "current edits"
    );
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

fn assert_unborn(directory: &Path, branch: &str) {
    assert_eq!(git(directory, &["symbolic-ref", "HEAD"]), branch);
    let output = Command::new("git")
        .current_dir(directory)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .assert_value();
    assert!(
        !output.status.success(),
        "unborn HEAD unexpectedly resolved"
    );
}

#[test]
fn unborn_snapshot_restores_staged_files_into_same_and_fresh_workspaces() {
    let fixture = Fixture::new();
    git(&fixture.workspace, &["init", "-b", "unborn-writer"]);
    fs::write(fixture.workspace.join("staged"), "checkpoint index").assert_value();
    git(&fixture.workspace, &["add", "staged"]);
    fs::write(fixture.workspace.join("staged"), "checkpoint working tree").assert_value();
    let id = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    assert_unborn(&fixture.tree(&id), "refs/heads/unborn-writer");

    git(
        &fixture.workspace,
        &["config", "checkpoint.authority", "later"],
    );
    fs::write(fixture.workspace.join("staged"), "later index").assert_value();
    git(&fixture.workspace, &["add", "staged"]);
    fs::write(fixture.workspace.join("later-only"), "remove").assert_value();
    restore(&fixture.snapshots, &id, &fixture.workspace).assert_value();
    assert_unborn(&fixture.workspace, "refs/heads/unborn-writer");
    assert_eq!(
        git(&fixture.workspace, &["show", ":staged"]),
        "checkpoint index"
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("staged")).assert_value(),
        "checkpoint working tree"
    );
    assert!(!fixture.workspace.join("later-only").exists());
    assert_eq!(
        git(&fixture.workspace, &["config", "checkpoint.authority"]),
        "later"
    );

    let fresh = fixture._root.path().join("fresh-unborn");
    restore(&fixture.snapshots, &id, &fresh).assert_value();
    assert_unborn(&fresh, "refs/heads/unborn-writer");
    assert_eq!(git(&fresh, &["show", ":staged"]), "checkpoint index");
    assert_eq!(git(&fresh, &["status", "--porcelain"]), "AM staged");
}

#[test]
fn empty_unborn_snapshot_restores_without_initializing_or_retaining_a_later_index() {
    let fixture = Fixture::new();
    git(&fixture.workspace, &["init", "-b", "main"]);
    let id = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    assert!(!fixture.tree(&id).join(".git/index").exists());
    fs::write(fixture.workspace.join("later"), "later staged file").assert_value();
    git(&fixture.workspace, &["add", "later"]);

    restore(&fixture.snapshots, &id, &fixture.workspace).assert_value();

    assert_unborn(&fixture.workspace, "refs/heads/main");
    assert!(!fixture.workspace.join(".git/index").exists());
    assert!(!fixture.workspace.join("later").exists());
    assert_eq!(git(&fixture.workspace, &["status", "--porcelain"]), "");
}

#[test]
fn unborn_snapshot_refuses_a_now_packed_branch_without_mutating_files_or_index() {
    let fixture = Fixture::new();
    git(&fixture.workspace, &["init", "-b", "main"]);
    fs::write(fixture.workspace.join("staged"), "unborn checkpoint").assert_value();
    git(&fixture.workspace, &["add", "staged"]);
    let id = capture(&fixture.workspace, &fixture.snapshots, &()).assert_value();
    git(&fixture.workspace, &["commit", "-m", "first commit"]);
    git(&fixture.workspace, &["pack-refs", "--all"]);
    assert!(!fixture.workspace.join(".git/refs/heads/main").exists());
    let shared_ref = git(&fixture.workspace, &["rev-parse", "refs/heads/main"]);
    fs::write(fixture.workspace.join("staged"), "current staged edit").assert_value();
    git(&fixture.workspace, &["add", "staged"]);
    fs::write(fixture.workspace.join("keep"), "current untracked edit").assert_value();
    let current_index = fs::read(fixture.workspace.join(".git/index")).assert_value();
    let current_head = fs::read(fixture.workspace.join(".git/HEAD")).assert_value();

    let error = restore(&fixture.snapshots, &id, &fixture.workspace).unwrap_err();
    assert!(error.to_string().contains("shared refs are preserved"));
    let stage = fixture.snapshots.join(id.as_str());
    assert!(restore_staged(&stage, &fixture.workspace).is_err());
    assert_eq!(
        git(&fixture.workspace, &["rev-parse", "refs/heads/main"]),
        shared_ref
    );
    assert_eq!(
        fs::read(fixture.workspace.join(".git/index")).assert_value(),
        current_index
    );
    assert_eq!(
        fs::read(fixture.workspace.join(".git/HEAD")).assert_value(),
        current_head
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("keep")).assert_value(),
        "current untracked edit"
    );
    assert_eq!(
        git(&fixture.workspace, &["show", ":staged"]),
        "current staged edit"
    );
    assert_eq!(
        fs::read_to_string(fixture.tree(&id).join("staged")).assert_value(),
        "unborn checkpoint"
    );
}

#[test]
fn unborn_linked_worktree_restore_preserves_its_admin_identity_and_shared_refs() {
    let fixture = Fixture::new();
    fixture.initialize_git();
    let linked = fixture._root.path().join("unborn-linked");
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
    git(&linked, &["switch", "--orphan", "unborn-writer"]);
    fs::write(linked.join("staged"), "checkpoint staged").assert_value();
    git(&linked, &["add", "staged"]);
    let marker = fs::read(linked.join(".git")).assert_value();
    let id = capture(&linked, &fixture.snapshots, &()).assert_value();

    fs::write(fixture.workspace.join("tracked"), "shared branch advanced").assert_value();
    git(
        &fixture.workspace,
        &["commit", "-am", "advance shared branch"],
    );
    let shared_ref = git(&fixture.workspace, &["rev-parse", "refs/heads/main"]);
    fs::write(linked.join("staged"), "later index").assert_value();
    git(&linked, &["add", "staged"]);
    restore(&fixture.snapshots, &id, &linked).assert_value();

    assert_unborn(&linked, "refs/heads/unborn-writer");
    assert_eq!(fs::read(linked.join(".git")).assert_value(), marker);
    assert_eq!(git(&linked, &["show", ":staged"]), "checkpoint staged");
    assert_eq!(
        git(&fixture.workspace, &["rev-parse", "refs/heads/main"]),
        shared_ref
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("tracked")).assert_value(),
        "shared branch advanced"
    );
    assert!(
        fixture
            .workspace
            .join(".git/worktrees/unborn-linked")
            .is_dir()
    );
}

#[test]
fn unborn_capture_refuses_malformed_symbolic_head_without_publishing() {
    for head in [
        "ref: refs/heads/../outside\n",
        "ref: refs/heads/main\nextra\n",
        "ref: refs/remotes/missing\n",
    ] {
        let fixture = Fixture::new();
        git(&fixture.workspace, &["init", "-b", "main"]);
        fs::write(fixture.workspace.join(".git/HEAD"), head).assert_value();
        fs::write(fixture.workspace.join("kept"), "caller bytes").assert_value();
        assert!(capture(&fixture.workspace, &fixture.snapshots, &()).is_err());
        assert_eq!(
            fs::read_to_string(fixture.workspace.join("kept")).assert_value(),
            "caller bytes"
        );
        assert_eq!(fs::read_dir(&fixture.snapshots).assert_value().count(), 0);
    }
}

#[cfg(unix)]
#[test]
fn unborn_capture_refuses_symlinked_head_without_touching_its_target() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    git(&fixture.workspace, &["init", "-b", "main"]);
    let external = fixture._root.path().join("external-head");
    fs::write(&external, "ref: refs/heads/main\n").assert_value();
    fs::remove_file(fixture.workspace.join(".git/HEAD")).assert_value();
    symlink(&external, fixture.workspace.join(".git/HEAD")).assert_value();
    assert!(capture(&fixture.workspace, &fixture.snapshots, &()).is_err());
    assert_eq!(
        fs::read_to_string(&external).assert_value(),
        "ref: refs/heads/main\n"
    );
    assert_eq!(fs::read_dir(&fixture.snapshots).assert_value().count(), 0);
}
