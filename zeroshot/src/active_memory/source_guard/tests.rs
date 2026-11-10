use super::*;

struct Repository(tempfile::TempDir);

impl Repository {
    fn new() -> Self {
        let repository = Self(tempfile::tempdir().unwrap());
        repository.git(&["init", "--quiet"]);
        repository.commit("initial", "initial");
        repository
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn git(&self, arguments: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(self.path())
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .args(arguments)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn commit(&self, contents: &str, message: &str) -> String {
        std::fs::write(self.path().join("source.rs"), contents).unwrap();
        self.git(&["add", "source.rs"]);
        self.git(&["commit", "--quiet", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }
}

fn lesson(revision: Option<&str>) -> Lesson {
    let mut references = vec!["v2:1".to_owned(), "repository:source.rs".to_owned()];
    if let Some(revision) = revision {
        references.push(format!("source:{revision}"));
    }
    serde_json::from_value(serde_json::json!({
        "schema_version":1,"id":"source-lesson","revision":1,
        "scope":{"kind":"repository","repository":"example/repo","files":["source.rs"],
            "folders":[],"identifiers":[],"keyword_groups":[]},
        "title":"Preserve the current publication path",
        "recommendation":"Use the verified publication helper at the owning boundary.",
        "evidence":{"run_id":"run","node_id":"worker","references":references,
            "costly_recurring_reason":"A repeated publication failure required an implementation repair."}
    })).unwrap()
}

#[tokio::test]
async fn compatible_ancestors_allow_changes_but_older_or_divergent_checkouts_do_not() {
    let repository = Repository::new();
    let base = repository.git(&["rev-parse", "HEAD"]);
    let newer = repository.commit("new helper", "add publication helper");
    let old = lesson(Some(&base));
    let supported = lesson(Some(&newer));
    let guard = SourceGuard::capture(repository.path(), &[old.clone(), supported.clone()]).await;
    assert!(guard.permits(&old));
    assert!(guard.permits(&supported));
    assert_eq!(guard.reference(), Some(format!("source:{newer}").as_str()));
    assert!(guard.unchanged(repository.path()).await);

    repository.git(&["checkout", "--quiet", "--detach", &base]);
    let older = SourceGuard::capture(repository.path(), std::slice::from_ref(&supported)).await;
    assert!(!older.permits(&supported));
    assert!(!guard.unchanged(repository.path()).await);
    // The same source tree in a squash/divergent commit is not ancestry evidence.
    repository.commit("new helper", "squashed or divergent implementation");
    let divergent = SourceGuard::capture(repository.path(), std::slice::from_ref(&supported)).await;
    assert!(!divergent.permits(&supported));
}

#[tokio::test]
async fn dirty_unknown_and_missing_source_never_gain_mutation_authority() {
    let repository = Repository::new();
    let revision = repository.git(&["rev-parse", "HEAD"]);
    let known = lesson(Some(&revision));
    let legacy = lesson(None);
    let missing = lesson(Some(&"a".repeat(40)));
    let clean = SourceGuard::capture(repository.path(), &[legacy.clone(), missing.clone()]).await;
    assert!(!clean.permits(&legacy));
    assert!(!clean.permits(&missing));
    std::fs::write(repository.path().join("source.rs"), "dirty").unwrap();
    let dirty = SourceGuard::capture(repository.path(), std::slice::from_ref(&known)).await;
    assert_eq!(dirty.reference(), None);
    assert!(!dirty.permits(&known));
    assert!(!clean.unchanged(repository.path()).await);
    repository.git(&["restore", "source.rs"]);
    std::fs::write(repository.path().join("untracked.rs"), "untracked").unwrap();
    assert_eq!(
        SourceGuard::capture(repository.path(), &[])
            .await
            .reference(),
        None
    );

    let plain = tempfile::tempdir().unwrap();
    let absent = SourceGuard::capture(plain.path(), std::slice::from_ref(&known)).await;
    assert_eq!(absent.reference(), None);
    assert!(!absent.permits(&known));
    let mut draft = legacy.draft();
    absent.stamp(&mut draft, |_| true).unwrap();
    assert!(
        draft
            .evidence
            .references
            .iter()
            .all(|value| !value.starts_with("source:"))
    );
}

#[tokio::test]
async fn host_stamping_retains_required_evidence_and_refuses_forged_provenance() {
    let repository = Repository::new();
    let guard = SourceGuard::capture(repository.path(), &[]).await;
    let mut draft = lesson(None).draft();
    draft
        .evidence
        .references
        .extend((2..8).map(|index| format!("v2:{index}")));
    assert_eq!(draft.evidence.references.len(), 8);
    guard
        .stamp(&mut draft, |references| {
            references.contains(&"v2:1".to_owned())
                && references.contains(&"repository:source.rs".to_owned())
        })
        .unwrap();
    assert_eq!(draft.evidence.references.len(), 8);
    assert!(draft.evidence.references.contains(&"v2:1".to_owned()));
    assert!(
        draft
            .evidence
            .references
            .contains(&"repository:source.rs".to_owned())
    );
    assert!(
        draft
            .evidence
            .references
            .iter()
            .any(|value| Some(value.as_str()) == guard.reference())
    );
    draft.validate().unwrap();
    assert!(guard.stamp(&mut draft, |_| true).is_err());

    let mut graph = lesson(None);
    graph.scope = LessonScope::Graph {
        graph_hash: "a".repeat(64),
        node_id: "worker".into(),
        repository: None,
    };
    assert!(SourceGuard::default().permits(&graph));
    let mut draft = graph.draft();
    guard.stamp(&mut draft, |_| true).unwrap();
    assert!(
        draft
            .evidence
            .references
            .iter()
            .all(|value| !value.starts_with("source:"))
    );
    graph.scope = LessonScope::Graph {
        graph_hash: "a".repeat(64),
        node_id: "worker".into(),
        repository: Some("example/repo".into()),
    };
    assert!(!SourceGuard::default().permits(&graph));
}

#[tokio::test]
async fn source_inspection_does_not_execute_workspace_filters_or_submodule_status() {
    let repository = Repository::new();
    repository.git(&["config", "filter.probe.clean", "echo ran > filter-ran; cat"]);
    std::fs::write(
        repository.path().join(".gitattributes"),
        "source.rs filter=probe\n",
    )
    .unwrap();
    // The filter is configured only after commit so fixture preparation does not execute it.
    std::fs::write(repository.path().join("source.rs"), "changed").unwrap();
    assert_eq!(
        SourceGuard::capture(repository.path(), &[])
            .await
            .reference(),
        None
    );
    assert!(!repository.path().join("filter-ran").exists());
    repository.git(&["config", "--unset", "filter.probe.clean"]);
    std::fs::remove_file(repository.path().join(".gitattributes")).unwrap();
    repository.git(&["restore", "source.rs"]);
    let revision = repository.git(&["rev-parse", "HEAD"]);
    repository.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{revision},nested"),
    ]);
    assert_eq!(
        SourceGuard::capture(repository.path(), &[])
            .await
            .reference(),
        None
    );
}

#[tokio::test]
async fn hidden_worktree_changes_cannot_claim_the_indexed_source_revision() {
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        let repository = Repository::new();
        let revision = repository.git(&["rev-parse", "HEAD"]);
        let original = lesson(Some(&revision));
        let clean = SourceGuard::capture(repository.path(), std::slice::from_ref(&original)).await;
        assert!(clean.permits(&original));
        repository.git(&["update-index", flag, "source.rs"]);
        std::fs::write(repository.path().join("source.rs"), "changed").unwrap();
        assert!(repository.git(&["status", "--porcelain=v1"]).is_empty());
        let hidden = SourceGuard::capture(repository.path(), std::slice::from_ref(&original)).await;
        assert_eq!(hidden.reference(), None, "hidden index flag {flag}");
        assert!(!hidden.permits(&original));
        assert!(!clean.unchanged(repository.path()).await);
    }
}

#[tokio::test]
async fn grafted_history_cannot_make_a_divergent_source_basis_compatible() {
    let repository = Repository::new();
    let base = repository.git(&["rev-parse", "HEAD"]);
    repository.git(&["checkout", "--quiet", "--orphan", "unrelated"]);
    let unrelated = repository.commit("unrelated source", "unrelated root");
    std::fs::write(
        repository.path().join(".git/info/grafts"),
        format!("{unrelated} {base}\n"),
    )
    .unwrap();
    // Ordinary Git sees the forged ancestry, but host inspection must ignore it.
    repository.git(&["merge-base", "--is-ancestor", &base, &unrelated]);
    let original = lesson(Some(&base));
    let guard = SourceGuard::capture(repository.path(), std::slice::from_ref(&original)).await;
    assert_eq!(
        guard.reference(),
        Some(format!("source:{unrelated}").as_str())
    );
    assert!(!guard.permits(&original));
}

#[tokio::test]
async fn bounded_source_inventory_handles_more_than_printable_diagnostic_capacity() {
    let repository = Repository::new();
    for index in 0..300 {
        std::fs::write(
            repository
                .path()
                .join(format!("{index:03}-{}.rs", "source".repeat(12))),
            "source",
        )
        .unwrap();
    }
    repository.git(&["add", "."]);
    repository.git(&["commit", "--quiet", "-m", "large source inventory"]);
    let indexed = repository.git(&["ls-files", "-v", "--stage", "-z"]);
    assert!(indexed.len() > 16 * 1024);
    let guard = SourceGuard::capture(repository.path(), &[]).await;
    assert!(guard.reference().is_some());
}

#[cfg(unix)]
#[tokio::test]
async fn repository_stat_policy_cannot_hide_same_length_source_edits() {
    let repository = Repository::new();
    repository.git(&["config", "core.trustctime", "false"]);
    repository.git(&["config", "core.checkStat", "minimal"]);
    let path = repository.path().join("source.rs");
    let previous = std::fs::metadata(&path).unwrap();
    // Move the index timestamp beyond the original file's timestamp, avoiding racy-clean checks.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    repository.git(&["update-index", "--force-write-index"]);
    std::fs::write(&path, "changed").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(previous.modified().unwrap())
                .set_accessed(previous.accessed().unwrap()),
        )
        .unwrap();
    assert!(repository.git(&["status", "--porcelain=v1"]).is_empty());
    assert!(repository.git(&["ls-files", "-v"]).starts_with("H "));
    assert_eq!(
        SourceGuard::capture(repository.path(), &[])
            .await
            .reference(),
        None
    );
}

#[cfg(unix)]
#[tokio::test]
async fn repository_filemode_and_symlink_settings_cannot_hide_changed_source() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let repository = Repository::new();
    repository.git(&["config", "core.fileMode", "false"]);
    std::fs::set_permissions(
        repository.path().join("source.rs"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(repository.git(&["status", "--porcelain=v1"]).is_empty());
    assert_eq!(
        SourceGuard::capture(repository.path(), &[])
            .await
            .reference(),
        None
    );

    let linked = Repository::new();
    let source = linked.path().join("source.rs");
    std::fs::write(linked.path().join("target.rs"), "target").unwrap();
    std::fs::remove_file(&source).unwrap();
    symlink("target.rs", &source).unwrap();
    linked.git(&["add", "."]);
    linked.git(&["commit", "--quiet", "-m", "linked source"]);
    linked.git(&["config", "core.symlinks", "false"]);
    std::fs::remove_file(&source).unwrap();
    std::fs::write(&source, "target.rs").unwrap();
    assert!(linked.git(&["status", "--porcelain=v1"]).is_empty());
    assert_eq!(
        SourceGuard::capture(linked.path(), &[]).await.reference(),
        None
    );
}

#[tokio::test]
async fn ignored_and_new_source_files_are_not_covered_by_a_clean_head_marker() {
    let repository = generated_repository();
    assert!(repository.git(&["status", "--porcelain=v1"]).is_empty());
    let guard = verified_guard(&repository).await;
    assert!(guard.permits_file(repository.path(), "source.rs"));
    assert!(!guard.permits_file(repository.path(), "generated/report.rs"));
    std::fs::write(repository.path().join("new.rs"), "new source").unwrap();
    assert!(!guard.permits_file(repository.path(), "new.rs"));
    assert!(!guard.unchanged(repository.path()).await);
}

#[cfg(unix)]
#[tokio::test]
async fn tracked_symlinks_require_a_tracked_canonical_source_target() {
    use std::os::unix::fs::symlink;
    let repository = generated_repository();
    symlink("source.rs", repository.path().join("tracked-link.rs")).unwrap();
    symlink(
        "generated/report.rs",
        repository.path().join("ignored-link.rs"),
    )
    .unwrap();
    repository.git(&["add", "."]);
    repository.git(&["commit", "--quiet", "-m", "source links"]);
    let guard = verified_guard(&repository).await;
    assert!(guard.permits_file(repository.path(), "tracked-link.rs"));
    assert!(!guard.permits_file(repository.path(), "ignored-link.rs"));
}

fn generated_repository() -> Repository {
    let repository = Repository::new();
    std::fs::write(repository.path().join(".gitignore"), "generated/\n").unwrap();
    repository.git(&["add", ".gitignore"]);
    repository.git(&["commit", "--quiet", "-m", "ignore generated output"]);
    std::fs::create_dir(repository.path().join("generated")).unwrap();
    std::fs::write(
        repository.path().join("generated/report.rs"),
        "generated authority",
    )
    .unwrap();
    repository
}

async fn verified_guard(repository: &Repository) -> SourceGuard {
    let guard = SourceGuard::capture(repository.path(), &[]).await;
    assert!(guard.reference().is_some());
    guard
}
