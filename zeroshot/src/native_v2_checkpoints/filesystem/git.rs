//! Git state owned by one workspace. Restoring a live repository never rewinds its shared refs,
//! configuration, or another worktree's administrative directory.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{
    copy_workspace_entry, entry_metadata, existing_directory, invalid, private_stage, remove_entry,
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
    existing_directory(&workspace.join(target))
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
        (true, false) => validate_linked_worktree(workspace, layout),
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

pub(super) fn capture(workspace: &Path, tree: &Path) -> io::Result<bool> {
    capture_tree(workspace, tree, None)
}

fn capture_tree(workspace: &Path, tree: &Path, modules: Option<&Path>) -> io::Result<bool> {
    let layout = GitLayout::discover(workspace, modules)?;
    let own_modules = layout
        .as_ref()
        .map(|layout| layout.directory.join("modules"));
    capture_nested(workspace, tree, own_modules.as_deref().or(modules))?;
    let Some(layout) = layout else {
        return Ok(false);
    };
    validate_git_metadata(&layout.common.join("objects"))?;
    let head = resolved_head(workspace, &layout)?;
    let target = tree.join(".git");
    normalize_git_tree(&layout, &target)?;
    validate_git_metadata(&target)?;
    write_detached_head(&target, &head)?;
    normalize_config(&target)?;
    Ok(true)
}

fn capture_nested(workspace: &Path, tree: &Path, modules: Option<&Path>) -> io::Result<()> {
    for entry in fs::read_dir(tree)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if entry.file_name() != ".git" && metadata.is_dir() && !metadata.file_type().is_symlink() {
            capture_tree(&workspace.join(entry.file_name()), &entry.path(), modules)?;
        }
    }
    Ok(())
}

fn write_detached_head(target: &Path, head: &str) -> io::Result<()> {
    remove_entry(&target.join("HEAD"))?;
    fs::write(target.join("HEAD"), format!("{head}\n"))
}

fn normalize_git_tree(layout: &GitLayout, target: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(target)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        return remove_entry(&target.join("worktrees"));
    }
    remove_entry(target)?;
    platform::create_private_directory(target)?;
    populate_git(layout, target)
}

fn populate_git(layout: &GitLayout, target: &Path) -> io::Result<()> {
    for path in SHARED_PATHS {
        copy_optional(&layout.common.join(path), &target.join(path))?;
    }
    copy_private(&layout.directory, target)?;
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

fn private_names(directory: &Path) -> io::Result<BTreeSet<std::ffi::OsString>> {
    fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .filter(|entry| entry.as_ref().map_or(true, |name| private_name(name)))
        .collect()
}

fn copy_private(source: &Path, target: &Path) -> io::Result<()> {
    let names = private_names(source)?
        .union(&private_names(target)?)
        .cloned()
        .collect::<Vec<_>>();
    for name in names {
        replace_optional(&source.join(&name), &target.join(name))?;
    }
    for name in PRIVATE_PATHS {
        replace_optional(&source.join(name), &target.join(name))?;
    }
    Ok(())
}

fn replace_optional(source: &Path, target: &Path) -> io::Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| invalid("Git entry has no parent"))?;
    ensure_object_directory(parent)?;
    remove_entry(target)?;
    copy_optional(source, target)
}

fn copy_optional(source: &Path, target: &Path) -> io::Result<()> {
    match fs::symlink_metadata(source) {
        Ok(_) => {
            let parent = target
                .parent()
                .ok_or_else(|| invalid("Git entry has no parent"))?;
            fs::create_dir_all(parent)?;
            copy_workspace_entry(source, target)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn validate_git_metadata(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(invalid("Git metadata cannot contain symbolic links"));
    }
    if path.ends_with("objects/info/alternates") || path.ends_with("objects/info/http-alternates") {
        return Err(invalid(
            "external Git object alternates are unsupported by workspace snapshots",
        ));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            validate_git_metadata(&entry?.path())?;
        }
    }
    Ok(())
}

fn resolved_head(workspace: &Path, layout: &GitLayout) -> io::Result<String> {
    let output = git(workspace)
        .arg("--git-dir")
        .arg(&layout.directory)
        .args(["rev-parse", "--verify", "HEAD"])
        .output()?;
    if !output.status.success() {
        return Err(invalid("workspace snapshots require a committed Git HEAD"));
    }
    let head = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    let head = head.trim();
    if !matches!(head.len(), 40 | 64) || !head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("invalid Git checkpoint revision"));
    }
    Ok(head.to_owned())
}

fn git(workspace: &Path) -> Command {
    let mut command = Command::new("git");
    let mut safe_directory = std::ffi::OsString::from("safe.directory=");
    safe_directory.push(workspace);
    command
        .current_dir(workspace)
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
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1");
    command
}

fn normalize_config(directory: &Path) -> io::Result<()> {
    for name in ["config", "config.worktree"] {
        let original = directory.join(name);
        if name != "config" && !original.exists() {
            continue;
        }
        // Edit a separate config copy: failed Git may have left a legitimate .lock to retain.
        let stage = private_stage(directory, ".checkpoint-config-")?;
        let path = stage.path().join("config");
        copy_optional(&original, &path)?;
        rewrite_config(&path, name == "config")?;
        fs::rename(path, original)?;
    }
    Ok(())
}

fn rewrite_config(path: &Path, standalone: bool) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| invalid("Git config has no parent"))?;
    let status = git(directory)
        .arg("--work-tree")
        .arg(directory)
        .args(["config", "--file"])
        .arg(path)
        .args(["--unset-all", "core.worktree"])
        .status()?;
    if !status.success() && status.code() != Some(5) {
        return Err(invalid(
            "Git checkpoint configuration could not be normalized",
        ));
    }
    if standalone {
        let status = git(directory)
            .arg("--work-tree")
            .arg(directory)
            .args(["config", "--file"])
            .arg(path)
            .args(["core.bare", "false"])
            .status()?;
        if !status.success() {
            return Err(invalid(
                "Git checkpoint configuration could not be normalized",
            ));
        }
    }
    Ok(())
}

pub(super) fn restore_target(
    workspace: &Path,
    snapshot_has_git: bool,
) -> io::Result<Option<GitLayout>> {
    let target = GitLayout::discover(workspace, None)?;
    if workspace.exists() {
        validate_nested_restore(workspace)?;
    }
    if target.is_some() && !snapshot_has_git {
        return Err(invalid(
            "a snapshot without Git state cannot replace an existing repository",
        ));
    }
    if let Some(layout) = target.as_ref() {
        validate_git_metadata(&layout.common.join("objects"))?;
    }
    Ok(target)
}

// Nested checkouts become independent repositories. Replacing one that owns linked peers would
// remove those peers' shared administrative state; refuse before changing any workspace bytes.
fn validate_nested_restore(workspace: &Path) -> io::Result<()> {
    for entry in fs::read_dir(workspace)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            require_no_nested_peers(&entry.path().join(".git"))?;
            validate_nested_restore(&entry.path())?;
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
    add_objects(&snapshot.join("objects"), &target.common.join("objects"))?;
    copy_private(snapshot, &target.directory)
}

fn add_objects(source: &Path, target: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            ensure_object_directory(&destination)?;
            add_objects(&entry.path(), &destination)?;
        } else {
            add_object(&entry.path(), &destination)?;
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

fn add_object(source: &Path, target: &Path) -> io::Result<()> {
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(invalid("Git object destination is not a plain file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            copy_workspace_entry(source, target)
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
