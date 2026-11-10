//! Local, host-inspected source provenance for repository lesson changes.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use tokio::time::Instant;

use super::{ActiveMemoryError, Lesson, LessonDraft, LessonScope};
use crate::native_v2_delivery::command::{self, GitCommandOutput};

const INSPECTION_LIMIT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(crate) struct SourceGuard {
    reference: Option<String>,
    compatible: BTreeSet<String>,
    tracked_paths: BTreeSet<String>,
}

impl SourceGuard {
    /// Only the actual clean workspace HEAD is authority, never an admitted or merged SHA.
    /// Missing objects, shallow/divergent history and exhausted inspection time stay unknown.
    pub(crate) async fn capture(workspace: &Path, lessons: &[Lesson]) -> Self {
        let deadline = Instant::now() + INSPECTION_LIMIT;
        let Some((head, tracked_paths)) = clean_source(workspace, deadline).await else {
            return Self::default();
        };
        let mut compatible = BTreeSet::from([head.clone()]);
        let bases = lessons
            .iter()
            .filter(|lesson| source_bound(&lesson.scope))
            .filter_map(source_revision)
            .collect::<BTreeSet<_>>();
        for base in bases {
            if Instant::now() >= deadline {
                break;
            }
            if git(
                workspace,
                &["merge-base", "--is-ancestor", base, &head],
                deadline,
            )
            .await
            .is_some_and(|output| output.diagnostic().exit_status == Some(0))
            {
                compatible.insert(base.to_owned());
            }
        }
        Self {
            reference: Some(format!("source:{head}")),
            compatible,
            tracked_paths,
        }
    }

    pub(crate) fn reference(&self) -> Option<&str> {
        self.reference.as_deref()
    }

    pub(crate) fn permits(&self, original: &Lesson) -> bool {
        !source_bound(&original.scope)
            || source_revision(original).is_some_and(|base| self.compatible.contains(base))
    }

    /// A clean source basis covers tracked source and tracked symlink targets, never generated files.
    pub(crate) fn permits_file(&self, workspace: &Path, relative: &str) -> bool {
        self.tracked_paths.contains(relative)
            && tracked_target(workspace, relative)
                .is_some_and(|target| self.tracked_paths.contains(&target))
    }

    /// A model cannot author this reference. Preserve the caller's required evidence when
    /// making room within the existing eight-reference card limit.
    pub(crate) fn stamp(
        &self,
        draft: &mut LessonDraft,
        preserves: impl Fn(&[String]) -> bool,
    ) -> Result<(), ActiveMemoryError> {
        let references = &mut draft.evidence.references;
        if references
            .iter()
            .any(|reference| reference.starts_with("source:"))
        {
            return Err(ActiveMemoryError::Invalid(
                "source provenance is host-owned".into(),
            ));
        }
        let Some(reference) = self.reference().filter(|_| source_bound(&draft.scope)) else {
            return Ok(());
        };
        if references.len() == 8 {
            let removable = (0..references.len()).rev().find(|&index| {
                let retained = references
                    .iter()
                    .enumerate()
                    .filter(|&(position, _)| position != index)
                    .map(|(_, reference)| reference.clone())
                    .collect::<Vec<_>>();
                preserves(&retained)
            });
            references.remove(removable.ok_or(ActiveMemoryError::Capacity)?);
        }
        if references.len() >= 8 {
            return Err(ActiveMemoryError::Capacity);
        }
        references.push(reference.to_owned());
        Ok(())
    }

    pub(crate) async fn unchanged(&self, workspace: &Path) -> bool {
        let Some(reference) = self.reference() else {
            return false;
        };
        clean_source(workspace, Instant::now() + INSPECTION_LIMIT)
            .await
            .is_some_and(|(head, _)| reference == format!("source:{head}"))
    }
}

pub(crate) fn source_bound(scope: &LessonScope) -> bool {
    matches!(
        scope,
        LessonScope::Repository { .. }
            | LessonScope::Graph {
                repository: Some(_),
                ..
            }
    )
}

fn source_revision(lesson: &Lesson) -> Option<&str> {
    let mut references = lesson
        .evidence
        .references
        .iter()
        .filter_map(|reference| reference.strip_prefix("source:"));
    let revision = references.next()?;
    (valid_revision(revision) && references.next().is_none()).then_some(revision)
}

fn valid_revision(value: &str) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

async fn clean_source(workspace: &Path, deadline: Instant) -> Option<(String, BTreeSet<String>)> {
    verified_root(workspace, deadline).await?;
    let tracked_paths = simple_worktree(workspace, deadline).await?;
    let revision = head(workspace, deadline).await?;
    let status = git(
        workspace,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=normal",
            "--ignore-submodules=none",
        ],
        deadline,
    )
    .await?;
    if status.diagnostic().exit_status != Some(0) || !status.stdout().is_empty() {
        return None;
    }
    (head(workspace, deadline).await? == revision).then_some((revision, tracked_paths))
}

async fn verified_root(workspace: &Path, deadline: Instant) -> Option<()> {
    let root = git(workspace, &["rev-parse", "--show-toplevel"], deadline).await?;
    let root = Path::new(root.stdout().trim()).canonicalize().ok()?;
    (root == workspace.canonicalize().ok()?).then_some(())
}

// `status` may execute clean filters or recurse into submodule repositories. Neither is
// needed to establish ordinary source provenance; leave these workspaces unverified.
async fn simple_worktree(workspace: &Path, deadline: Instant) -> Option<BTreeSet<String>> {
    let filters = git(
        workspace,
        &[
            "config",
            "--name-only",
            "--get-regexp",
            r"^filter\..*\.(clean|process)$",
        ],
        deadline,
    )
    .await?;
    if filters.diagnostic().exit_status != Some(1) || !filters.stdout().is_empty() {
        return None;
    }
    let indexed = git(workspace, &["ls-files", "-v", "--stage", "-z"], deadline).await?;
    indexed_paths(&indexed)
}

fn indexed_paths(indexed: &GitCommandOutput) -> Option<BTreeSet<String>> {
    if indexed.diagnostic().exit_status != Some(0) {
        return None;
    }
    indexed
        .stdout()
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            // Lowercase tags hide assume-unchanged entries; S hides skip-worktree entries.
            if !entry.starts_with("H ") || entry.starts_with("H 160000 ") {
                return None;
            }
            let (_, path) = entry.split_once('\t')?;
            // Capture represents protocol stdout as UTF-8. Never treat a lossy filename as authority.
            (!path.contains('\u{fffd}')).then(|| path.to_owned())
        })
        .collect()
}

fn tracked_target(workspace: &Path, relative: &str) -> Option<String> {
    super::validate_relative_path(relative).ok()?;
    let root = workspace.canonicalize().ok()?;
    let canonical = root.join(relative).canonicalize().ok()?;
    let relative = canonical.strip_prefix(root).ok()?;
    let components = relative
        .iter()
        .map(|component| component.to_str())
        .collect::<Option<Vec<_>>>()?;
    Some(components.join("/"))
}

async fn head(workspace: &Path, deadline: Instant) -> Option<String> {
    let output = git(
        workspace,
        &["rev-parse", "--verify", "HEAD^{commit}"],
        deadline,
    )
    .await?;
    let revision = output.stdout().trim();
    (output.diagnostic().exit_status == Some(0) && valid_revision(revision))
        .then(|| revision.to_owned())
}

async fn git(workspace: &Path, args: &[&str], deadline: Instant) -> Option<GitCommandOutput> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    let mut command = command::local_git_command(Path::new("git"), workspace);
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env(
            "GIT_GRAFT_FILE",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.trustctime=true",
            "-c",
            "core.checkStat=default",
            "-c",
            "core.ignoreStat=false",
            "-c",
            "core.fileMode=true",
            "-c",
            "core.symlinks=true",
            "-c",
            "core.untrackedCache=false",
        ])
        .args(args);
    let output =
        command::capture_sensitive_with_stdout_limit(&mut command, remaining, &[], 1024 * 1024)
            .await
            .ok()?;
    (!output.stdout_truncated()).then_some(output)
}

#[cfg(test)]
mod tests;
