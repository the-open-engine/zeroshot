use std::path::{Path, PathBuf};

use openengine_cluster_testkit::assertions::AssertValue;

use super::{canonical_future_path, resolve_workspace, validate_workspace_state};
use crate::native_v2_local::LocalCompositionError;

struct Fixture {
    root: tempfile::TempDir,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().assert_value();
        let workspace = root.path().join("work");
        std::fs::create_dir(&workspace).assert_value();
        Self { root, workspace }
    }

    fn state(&self) -> PathBuf {
        self.root.path().join("state/new")
    }
}

#[test]
fn plain_directory_needs_no_git_executable() {
    let fixture = Fixture::new();
    assert_eq!(
        resolve_workspace(&fixture.workspace, Path::new("/unavailable/git")).assert_value(),
        std::fs::canonicalize(&fixture.workspace).assert_value()
    );
    assert!(!fixture.workspace.join(".git").exists());
}

#[test]
fn rejects_a_file_as_the_working_folder() {
    let fixture = Fixture::new();
    let file = fixture.workspace.join("file");
    std::fs::write(&file, "data").assert_value();
    assert!(matches!(
        resolve_workspace(&file, Path::new("/unavailable/git")),
        Err(LocalCompositionError::Workspace)
    ));
}

#[test]
fn disjoint_missing_state_remains_missing_after_preflight() {
    let fixture = Fixture::new();
    validate_workspace_state(&fixture.workspace, &fixture.state()).assert_value();
    assert!(!fixture.state().exists());
}

#[test]
fn state_equal_to_workspace_is_rejected() {
    let fixture = Fixture::new();
    assert_overlap(&fixture.workspace, &fixture.workspace);
}

#[test]
fn state_inside_workspace_is_rejected_without_creating_it() {
    let fixture = Fixture::new();
    let state = fixture.workspace.join("missing/state");
    assert_overlap(&fixture.workspace, &state);
    assert!(!fixture.workspace.join("missing").exists());
}

#[test]
fn workspace_inside_state_is_rejected() {
    let fixture = Fixture::new();
    assert_overlap(&fixture.workspace, fixture.root.path());
}

#[test]
fn parent_components_in_missing_paths_do_not_hide_overlap() {
    let fixture = Fixture::new();
    assert_overlap(
        &fixture.workspace,
        &fixture.workspace.join("missing/../state"),
    );
    assert_eq!(
        canonical_future_path(&fixture.workspace.join("missing/../../state")).assert_value(),
        std::fs::canonicalize(fixture.root.path())
            .assert_value()
            .join("state")
    );
}

#[cfg(unix)]
#[test]
fn symlink_state_prefix_is_resolved_before_a_missing_leaf() {
    let fixture = Fixture::new();
    let alias = fixture.root.path().join("alias");
    std::os::unix::fs::symlink(&fixture.workspace, &alias).assert_value();
    assert_overlap(&fixture.workspace, &alias.join("missing/state"));
}

#[cfg(unix)]
#[test]
fn parent_component_follows_the_resolved_symlink_target() {
    let fixture = Fixture::new();
    let nested = fixture.workspace.join("nested");
    std::fs::create_dir(&nested).assert_value();
    let alias = fixture.root.path().join("alias");
    std::os::unix::fs::symlink(&nested, &alias).assert_value();
    assert_overlap(&fixture.workspace, &alias.join("../state"));
}

#[cfg(unix)]
#[test]
fn dangling_state_symlink_fails_closed() {
    let fixture = Fixture::new();
    let alias = fixture.root.path().join("alias");
    std::os::unix::fs::symlink(fixture.root.path().join("missing"), &alias).assert_value();
    assert!(validate_workspace_state(&fixture.workspace, &alias.join("state")).is_err());
    assert!(!fixture.root.path().join("missing").exists());
}

fn assert_overlap(workspace: &Path, state: &Path) {
    assert!(matches!(
        validate_workspace_state(workspace, state),
        Err(LocalCompositionError::WorkspaceStateOverlap { .. })
    ));
}
