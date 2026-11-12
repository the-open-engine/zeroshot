//! Git state owned by one workspace. Restoring a live repository never rewinds its shared refs,
//! configuration, or another worktree's administrative directory.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{
    SnapshotOperation, entry_metadata, existing_directory, invalid, private_stage, remove_entry,
    require_directory,
};
use crate::execution::platform;

const SHARED_PATHS: &[&str] = &[
    "objects",
    "refs",
    "packed-refs",
    "config",
    "description",
    "info",
    "hooks",
    "shallow",
    "rr-cache",
];
const PRIVATE_PATHS: &[&str] = &[
    "info/sparse-checkout",
    "logs/HEAD",
    "logs/refs/bisect",
    "logs/refs/rewritten",
    "logs/refs/worktree",
    "refs/bisect",
    "refs/rewritten",
    "refs/worktree",
];

pub(super) struct GitLayout {
    directory: PathBuf,
    common: PathBuf,
}

impl GitLayout {
    fn discover(workspace: &Path, modules: Option<&Path>) -> io::Result<Option<Self>> {
        let marker = workspace.join(".git");
        let metadata = match fs::symlink_metadata(&marker) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if metadata.is_file() && metadata.len() == 0 {
            return Ok(None);
        }
        let directory = git_directory(workspace, &marker, &metadata)?;
        let common = common_directory(&directory)?;
        let layout = Self { directory, common };
        validate_layout(workspace, &layout, metadata.is_file(), modules)?;
        Ok(Some(layout))
    }
}

fn git_directory(workspace: &Path, marker: &Path, metadata: &fs::Metadata) -> io::Result<PathBuf> {
    if metadata.file_type().is_symlink() {
        return Err(invalid("symlinked Git directories are unsupported"));
    }
    if metadata.is_dir() {
        return existing_directory(marker);
    }
    if !metadata.is_file() {
        return Err(invalid("Git marker is not a plain file or directory"));
    }
    let marker = read_small(marker)?;
    let target = marker
        .trim()
        .strip_prefix("gitdir: ")
        .ok_or_else(|| invalid("invalid Git directory marker"))?;
    existing_directory(&workspace.join(target)).map_err(worktree_link_error)
}

fn common_directory(directory: &Path) -> io::Result<PathBuf> {
    let path = directory.join("commondir");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            existing_directory(&directory.join(read_small(&path)?.trim()))
        }
        Ok(_) => Err(invalid("Git common directory marker is not a plain file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(directory.to_owned()),
        Err(error) => Err(error),
    }
}

fn validate_layout(
    workspace: &Path,
    layout: &GitLayout,
    gitfile: bool,
    modules: Option<&Path>,
) -> io::Result<()> {
    let GitLayout { directory, common } = layout;
    match (gitfile, directory == common) {
        (false, true) => Ok(()),
        (true, true) if modules.is_some_and(|root| directory.starts_with(root)) => Ok(()),
        (true, false) => validate_linked_worktree(workspace, layout).map_err(worktree_link_error),
        _ => Err(invalid(
            "Git administrative path is outside this workspace or its linked worktree",
        )),
    }
}

fn validate_linked_worktree(workspace: &Path, layout: &GitLayout) -> io::Result<()> {
    let GitLayout { directory, common } = layout;
    if directory.parent() != Some(common.join("worktrees").as_path()) {
        return Err(invalid("Git administrative path is not a linked worktree"));
    }
    let pointer = directory.join("gitdir");
    let metadata = fs::symlink_metadata(&pointer)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("Git worktree backpointer is not a plain file"));
    }
    let marker = fs::canonicalize(directory.join(read_small(&pointer)?.trim()))?;
    if marker != fs::canonicalize(workspace.join(".git"))? {
        return Err(invalid(
            "Git worktree backpointer does not name this workspace",
        ));
    }
    Ok(())
}

// Moving a worktree or its main checkout leaves links that Git can still repair, but a snapshot
// needs both to resolve. `git worktree repair` rewrites them; a stale checkout can be removed.
fn worktree_link_error(error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "unresolved Git worktree link: {error}; run `git worktree repair` with this checkout's \
             path from its main worktree, or remove the stale checkout"
        ),
    )
}

// Nested checkouts are named relative to the workspace; the workspace in the spelling Git accepts.
fn checkout_error(root: &Path, checkout: &Path, error: io::Error) -> io::Error {
    let path = match checkout.strip_prefix(root) {
        Ok(nested) if !nested.as_os_str().is_empty() => nested,
        _ => checkout,
    };
    io::Error::new(
        error.kind(),
        format!("Git checkout {:?}: {error}", git_path(path)),
    )
}

pub(super) fn capture(
    workspace: &Path,
    tree: &Path,
    operation: &SnapshotOperation,
) -> io::Result<bool> {
    capture_tree(
        &CaptureContext {
            root: workspace,
            operation,
        },
        workspace,
        tree,
        None,
    )
}

struct CaptureContext<'a> {
    root: &'a Path,
    operation: &'a SnapshotOperation,
}

fn capture_tree(
    context: &CaptureContext<'_>,
    workspace: &Path,
    tree: &Path,
    modules: Option<&Path>,
) -> io::Result<bool> {
    let CaptureContext { root, operation } = context;
    operation.check()?;
    let layout = GitLayout::discover(workspace, modules)
        .map_err(|error| checkout_error(root, workspace, error))?;
    let own_modules = layout
        .as_ref()
        .map(|layout| layout.directory.join("modules"));
    capture_nested(context, workspace, tree, own_modules.as_deref().or(modules))?;
    let Some(layout) = layout else {
        return Ok(false);
    };
    validate_git_metadata(&layout.common.join("objects"), operation)?;
    let head = checkpoint_head(workspace, &layout, operation)?;
    let target = tree.join(".git");
    normalize_git_tree(&layout, &target, operation)?;
    validate_git_metadata(&target, operation)?;
    write_snapshot_head(&target, &head)?;
    normalize_config(&target, operation)?;
    Ok(true)
}

fn capture_nested(
    context: &CaptureContext<'_>,
    workspace: &Path,
    tree: &Path,
    modules: Option<&Path>,
) -> io::Result<()> {
    for entry in fs::read_dir(tree)? {
        context.operation.check()?;
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if entry.file_name() != ".git" && metadata.is_dir() && !metadata.file_type().is_symlink() {
            capture_tree(
                context,
                &workspace.join(entry.file_name()),
                &entry.path(),
                modules,
            )?;
        }
    }
    Ok(())
}

fn write_snapshot_head(target: &Path, head: &str) -> io::Result<()> {
    remove_entry(&target.join("HEAD"))?;
    fs::write(target.join("HEAD"), format!("{head}\n"))
}

fn normalize_git_tree(
    layout: &GitLayout,
    target: &Path,
    operation: &SnapshotOperation,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(target)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        return remove_entry(&target.join("worktrees"));
    }
    remove_entry(target)?;
    platform::create_private_directory(target)?;
    populate_git(layout, target, operation)
}

fn populate_git(
    layout: &GitLayout,
    target: &Path,
    operation: &SnapshotOperation,
) -> io::Result<()> {
    for path in SHARED_PATHS {
        copy_optional(&layout.common.join(path), &target.join(path), operation)?;
    }
    copy_private(&layout.directory, target, operation)?;
    Ok(())
}

fn private_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let name = name.strip_suffix(".lock").unwrap_or(name);
    matches!(
        name,
        "index" | "config.worktree" | "rebase-apply" | "rebase-merge" | "sequencer"
    ) || name.starts_with("sharedindex.")
        || (!name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte == b'_'))
}

fn private_names(
    directory: &Path,
    operation: &SnapshotOperation,
) -> io::Result<BTreeSet<std::ffi::OsString>> {
    fs::read_dir(directory)?
        .map(|entry| {
            operation.check()?;
            entry.map(|entry| entry.file_name())
        })
        .filter(|entry| entry.as_ref().map_or(true, |name| private_name(name)))
        .collect()
}

fn copy_private(source: &Path, target: &Path, operation: &SnapshotOperation) -> io::Result<()> {
    copy_private_head(source, target, false, operation)
}

fn copy_private_head(
    source: &Path,
    target: &Path,
    preserve_head: bool,
    operation: &SnapshotOperation,
) -> io::Result<()> {
    let names = private_names(source, operation)?
        .union(&private_names(target, operation)?)
        .cloned()
        .collect::<Vec<_>>();
    for name in names {
        operation.check()?;
        if preserve_head && name == "HEAD" {
            continue;
        }
        replace_optional(&source.join(&name), &target.join(name), operation)?;
    }
    for name in PRIVATE_PATHS {
        operation.check()?;
        replace_optional(&source.join(name), &target.join(name), operation)?;
    }
    Ok(())
}

fn replace_optional(source: &Path, target: &Path, operation: &SnapshotOperation) -> io::Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| invalid("Git entry has no parent"))?;
    ensure_object_directory(parent)?;
    match fs::symlink_metadata(source) {
        // HEAD and the index must never be absent: an interrupted restore is retried in this
        // checkout, and Git refuses a directory without HEAD.
        Ok(metadata) if metadata.is_file() => publish_file(source, target, operation),
        Ok(_) => {
            remove_entry(target)?;
            copy_optional(source, target, operation)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => remove_entry(target),
        Err(error) => Err(error),
    }
}

/// Installs a complete copy of `source` at `target` with one rename, so an interrupted restore
/// leaves either the previous file or the new one. The temporary uses Git's prefix for temporary
/// objects, which `git fsck` tolerates and `git prune` removes from object directories. A hard
/// crash between copy and rename can still leave one inert temporary in the administrative
/// directory, which Git ignores.
fn publish_file(source: &Path, target: &Path, operation: &SnapshotOperation) -> io::Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| invalid("Git entry has no parent"))?;
    if entry_metadata(target)?.is_some_and(|metadata| metadata.is_dir()) {
        remove_entry(target)?;
    }
    let temporary = parent.join(format!("tmp_obj_{}", uuid::Uuid::now_v7().simple()));
    let result = operation
        .copy(source, &temporary)
        .and_then(|()| fs::rename(&temporary, target));
    if result.is_err() {
        let _ = remove_entry(&temporary);
    }
    result
}

fn copy_optional(source: &Path, target: &Path, operation: &SnapshotOperation) -> io::Result<()> {
    match fs::symlink_metadata(source) {
        Ok(_) => {
            let parent = target
                .parent()
                .ok_or_else(|| invalid("Git entry has no parent"))?;
            fs::create_dir_all(parent)?;
            operation.copy(source, target)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn validate_git_metadata(path: &Path, operation: &SnapshotOperation) -> io::Result<()> {
    operation.check()?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(invalid("Git metadata cannot contain symbolic links"));
    }
    if (path.ends_with("objects/info/alternates") || path.ends_with("objects/info/http-alternates"))
        && metadata.len() > 0
    {
        return Err(invalid(
            "external Git object alternates are unsupported by workspace snapshots",
        ));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            validate_git_metadata(&entry?.path(), operation)?;
        }
    }
    Ok(())
}

fn checkpoint_head(
    workspace: &Path,
    layout: &GitLayout,
    operation: &SnapshotOperation,
) -> io::Result<String> {
    require_plain_head(&layout.directory)?;
    let mut command = git_directory_command(workspace, &layout.directory);
    command.args(["rev-parse", "--verify", "HEAD"]);
    let output = operation.git(command)?;
    if output.diagnostic().exit_status != Some(0) {
        return unborn_head(workspace, layout, operation);
    }
    let head = output.stdout().trim();
    if !checkpoint_revision(head) {
        return Err(invalid("invalid Git checkpoint revision"));
    }
    Ok(head.to_owned())
}

fn checkpoint_revision(head: &str) -> bool {
    matches!(head.len(), 40 | 64) && head.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn require_plain_head(directory: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(directory.join("HEAD"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("Git HEAD must be a plain file"));
    }
    Ok(())
}

fn unborn_head(
    workspace: &Path,
    layout: &GitLayout,
    operation: &SnapshotOperation,
) -> io::Result<String> {
    let mut command = git_directory_command(workspace, &layout.directory);
    command.args(["symbolic-ref", "--quiet", "--no-recurse", "HEAD"]);
    let output = operation.git(command)?;
    if output.diagnostic().exit_status != Some(0) {
        return Err(invalid(
            "Git checkpoint HEAD is neither committed nor unborn",
        ));
    }
    let reference = output.stdout().trim();
    require_branch_reference(&layout.directory, reference, operation)?;
    require_missing_reference(workspace, layout, reference, operation)?;
    Ok(format!("ref: {reference}"))
}

fn require_branch_reference(
    directory: &Path,
    reference: &str,
    operation: &SnapshotOperation,
) -> io::Result<()> {
    if !reference.starts_with("refs/heads/") {
        return Err(invalid("invalid unborn Git checkpoint branch"));
    }
    let mut command = git_command(directory, None);
    command.args(["check-ref-format", reference]);
    if operation.git(command)?.diagnostic().exit_status != Some(0) {
        return Err(invalid("invalid unborn Git checkpoint branch"));
    }
    Ok(())
}

fn require_missing_reference(
    workspace: &Path,
    layout: &GitLayout,
    reference: &str,
    operation: &SnapshotOperation,
) -> io::Result<()> {
    let mut command = git_directory_command(workspace, &layout.directory);
    command.args(["show-ref", "--verify", "--quiet", reference]);
    let status = operation.git(command)?.diagnostic().exit_status;
    // Git resolves packed and backend-owned refs. The loose-path check additionally rejects a
    // broken or indirect ref that Git can report as missing; it is never the absence authority.
    if status != Some(1) || entry_metadata(&layout.common.join(reference))?.is_some() {
        return Err(invalid(
            "unborn Git checkpoint branch now exists or cannot be verified; shared refs are preserved",
        ));
    }
    Ok(())
}

fn snapshot_unborn_reference(
    directory: &Path,
    operation: &SnapshotOperation,
) -> io::Result<Option<String>> {
    require_plain_head(directory)?;
    let head = read_small(&directory.join("HEAD"))?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref: ") {
        require_branch_reference(directory, reference, operation)?;
        return Ok(Some(reference.to_owned()));
    }
    if !checkpoint_revision(head) {
        return Err(invalid("invalid Git checkpoint HEAD"));
    }
    Ok(None)
}

fn git_directory_command(workspace: &Path, directory: &Path) -> Command {
    let mut command = git(workspace);
    command
        .arg("--git-dir")
        .arg(git_path(directory).as_os_str());
    command
}

fn git(workspace: &Path) -> Command {
    git_command(workspace, Some(workspace))
}

fn git_command(workspace: &Path, working_directory: Option<&Path>) -> Command {
    let mut command = Command::new("git");
    let mut safe_directory = std::ffi::OsString::from("safe.directory=");
    safe_directory.push(git_path(workspace).as_os_str());
    if let Some(working_directory) = working_directory {
        command.current_dir(working_directory);
    }
    command
        .arg("-c")
        .arg(safe_directory)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        command.env_remove(name);
    }
    #[cfg(windows)]
    command.args(["-c", "core.longpaths=true"]);
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1");
    command
}

// Windows filesystem authority keeps its verbatim paths. Git rejects their `?` prefix in
// path arguments; use the equivalent DOS/UNC spelling and let core.longpaths handle Win32 I/O.
fn git_path(path: &Path) -> std::borrow::Cow<'_, Path> {
    #[cfg(not(windows))]
    return std::borrow::Cow::Borrowed(path);
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};

        let mut components = path.components();
        let Some(Component::Prefix(prefix)) = components.next() else {
            return std::borrow::Cow::Borrowed(path);
        };
        let mut plain = match prefix.kind() {
            Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:", char::from(drive))),
            Prefix::VerbatimUNC(server, share) => {
                let mut prefix = std::ffi::OsString::from(r"\\");
                prefix.push(server);
                prefix.push(r"\");
                prefix.push(share);
                PathBuf::from(prefix)
            }
            _ => return std::borrow::Cow::Borrowed(path),
        };
        plain.push(components.as_path());
        std::borrow::Cow::Owned(plain)
    }
}

fn normalize_config(directory: &Path, operation: &SnapshotOperation) -> io::Result<()> {
    for name in ["config", "config.worktree"] {
        operation.check()?;
        let original = directory.join(name);
        if name != "config" && !original.exists() {
            continue;
        }
        // Edit a separate config copy: failed Git may have left a legitimate .lock to retain.
        let stage = private_stage(directory, ".checkpoint-config-")?;
        let path = stage.path().join("config");
        copy_optional(&original, &path, operation)?;
        rewrite_config(&path, name == "config", operation)?;
        fs::rename(path, original)?;
    }
    Ok(())
}

fn rewrite_config(path: &Path, standalone: bool, operation: &SnapshotOperation) -> io::Result<()> {
    let mut command = git_config(path)?;
    command.args(["--unset-all", "core.worktree"]);
    let status = operation.git(command)?.diagnostic().exit_status;
    if !matches!(status, Some(0 | 5)) {
        return Err(invalid(
            "Git checkpoint configuration could not be normalized",
        ));
    }
    if standalone {
        let mut command = git_config(path)?;
        command.args(["core.bare", "false"]);
        if operation.git(command)?.diagnostic().exit_status != Some(0) {
            return Err(invalid(
                "Git checkpoint configuration could not be normalized",
            ));
        }
    }
    Ok(())
}

fn git_config(path: &Path) -> io::Result<Command> {
    let directory = path
        .parent()
        .ok_or_else(|| invalid("Git config has no parent"))?;
    // The config path is explicit. Avoid making the disposable capture tree the process working
    // directory: CreateProcessW rejects working directories longer than MAX_PATH even when Git
    // itself supports long paths.
    let mut command = git_command(directory, None);
    command
        .args(["config", "--no-includes", "--file"])
        .arg(git_path(path).as_os_str());
    Ok(command)
}

pub(super) fn restore_target(
    workspace: &Path,
    snapshot_git: Option<&Path>,
    operation: &SnapshotOperation,
) -> io::Result<Option<GitLayout>> {
    let unborn_reference = snapshot_git
        .map(|directory| snapshot_unborn_reference(directory, operation))
        .transpose()?
        .flatten();
    let target = GitLayout::discover(workspace, None)
        .map_err(|error| checkout_error(workspace, workspace, error))?;
    if workspace.exists() {
        validate_nested_restore(workspace, operation)?;
    }
    if target.is_some() && snapshot_git.is_none() {
        return Err(invalid(
            "a snapshot without Git state cannot replace an existing repository",
        ));
    }
    if let Some(layout) = target.as_ref() {
        validate_existing_restore(workspace, layout, unborn_reference.as_deref(), operation)?;
    }
    Ok(target)
}

fn validate_existing_restore(
    workspace: &Path,
    layout: &GitLayout,
    unborn_reference: Option<&str>,
    operation: &SnapshotOperation,
) -> io::Result<()> {
    validate_git_metadata(&layout.common.join("objects"), operation)?;
    if let Some(reference) = unborn_reference {
        require_missing_reference(workspace, layout, reference, operation)?;
    }
    Ok(())
}

// Nested checkouts become independent repositories. Replacing one that owns linked peers would
// remove those peers' shared administrative state; refuse before changing any workspace bytes.
fn validate_nested_restore(workspace: &Path, operation: &SnapshotOperation) -> io::Result<()> {
    for entry in fs::read_dir(workspace)? {
        operation.check()?;
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            require_no_nested_peers(&entry.path().join(".git"))?;
            validate_nested_restore(&entry.path(), operation)?;
        }
    }
    Ok(())
}

fn require_no_nested_peers(directory: &Path) -> io::Result<()> {
    let Some(metadata) = entry_metadata(directory)? else {
        return Ok(());
    };
    if !metadata.file_type().is_dir() {
        return Ok(());
    }
    let peers = directory.join("worktrees");
    let Some(metadata) = entry_metadata(&peers)? else {
        return Ok(());
    };
    require_directory(&metadata)?;
    if fs::read_dir(peers)?.next().transpose()?.is_some() {
        return Err(invalid(
            "restoring a nested repository with linked worktrees is unsupported",
        ));
    }
    Ok(())
}

pub(super) fn restore_existing(snapshot: &Path, target: &GitLayout) -> io::Result<()> {
    // Once replacement starts, finish installing the snapshot even if shutdown is requested.
    let operation = SnapshotOperation::uncancelled();
    add_objects(
        &snapshot.join("objects"),
        &target.common.join("objects"),
        &operation,
    )?;
    // Restored files and index may describe an older candidate. Keep an existing attached HEAD
    // at its live branch tip: attaching the snapshot's commit would require rewinding shared refs.
    let preserve_head = snapshot_unborn_reference(snapshot, &operation)?.is_none()
        && require_plain_head(&target.directory).is_ok()
        && read_small(&target.directory.join("HEAD"))?.starts_with("ref: refs/heads/");
    copy_private_head(snapshot, &target.directory, preserve_head, &operation)
}

fn add_objects(source: &Path, target: &Path, operation: &SnapshotOperation) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            ensure_object_directory(&destination)?;
            add_objects(&entry.path(), &destination, operation)?;
        } else {
            add_object(&entry.path(), &destination, operation)?;
        }
    }
    Ok(())
}

fn ensure_object_directory(path: &Path) -> io::Result<()> {
    match entry_metadata(path)? {
        Some(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Some(_) => Err(invalid("Git destination is not a plain directory")),
        None => fs::create_dir(path),
    }
}

fn add_object(source: &Path, target: &Path, operation: &SnapshotOperation) -> io::Result<()> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(invalid("Git object destination is not a plain file")),
        // A complete object is never replaced; publication keeps every new one complete too.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            publish_file(source, target, operation)
        }
        Err(error) => Err(error),
    }
}

fn read_small(path: &Path) -> io::Result<String> {
    let mut value = String::new();
    fs::File::open(path)?
        .take(16_385)
        .read_to_string(&mut value)?;
    if value.len() > 16_384 {
        return Err(invalid("Git administrative marker exceeds its bound"));
    }
    Ok(value)
}

#[cfg(all(test, windows))]
#[test]
fn git_arguments_normalize_only_windows_verbatim_prefixes() {
    for (input, expected) in [
        (r"\\?\C:\work space\λ\.git", r"C:\work space\λ\.git"),
        (
            r"\\?\UNC\server\share\work space",
            r"\\server\share\work space",
        ),
        (r"C:\work space\λ\.git", r"C:\work space\λ\.git"),
        (r"\\server\share\work space", r"\\server\share\work space"),
        (r"relative\directory", r"relative\directory"),
    ] {
        assert_eq!(git_path(Path::new(input)).as_ref(), Path::new(expected));
    }
}
