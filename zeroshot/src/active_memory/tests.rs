use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use super::*;

#[cfg(windows)]
#[test]
fn native_windows_paths_match_portable_repository_associations() {
    let root = std::path::Path::new(r"C:\checkout");
    assert_eq!(
        normalize_path(root, root, r"C:\checkout\src\guard.rs"),
        Some("src/guard.rs".into())
    );
    assert_eq!(
        normalize_path(root, root, r"src\guard.rs"),
        Some("src/guard.rs".into())
    );
    assert!(normalize_path(root, root, r"C:\outside\guard.rs").is_none());
}

struct PrivateRoot {
    _directory: tempfile::TempDir,
    path: PathBuf,
}

impl PrivateRoot {
    fn path(&self) -> &Path {
        &self.path
    }
}

fn private_root() -> PrivateRoot {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("private");
    crate::execution::platform::create_private_directory(&path).unwrap();
    PrivateRoot {
        _directory: directory,
        path,
    }
}

fn workspace() -> PathBuf {
    if cfg!(windows) {
        "C:/workspace/repo".into()
    } else {
        "/workspace/repo".into()
    }
}

fn draft(id: &str) -> LessonDraft {
    LessonDraft {
        id: id.into(),
        scope: LessonScope::Repository {
            repository: "example/zeroshot".into(),
            files: vec!["zeroshot/src/claude.rs".into()],
            folders: Vec::new(),
            identifiers: Vec::new(),
            keyword_groups: Vec::new(),
        },
        title: "Check every provider selector".into(),
        recommendation: "Reject competing provider selectors and refresh credentials before constructing the Bedrock environment.".into(),
        evidence: LessonEvidence {
            run_id: "run-1".into(),
            node_id: "worker".into(),
            references: vec!["event:42".into(), "test:provider_environment".into()],
            costly_recurring_reason: "A focused matrix omitted sibling controls and required another review and repair cycle.".into(),
        },
    }
}

fn create(draft: LessonDraft) -> LessonMutation {
    LessonMutation::Upsert {
        expected_revision: None,
        draft: Box::new(draft),
    }
}

fn repo_query() -> ScopeQuery {
    ScopeQuery {
        repository: Some("example/zeroshot".into()),
        repository_lessons: true,
        ..Default::default()
    }
}

fn index(drafts: Vec<LessonDraft>) -> MemoryIndex {
    MemoryIndex::new(MemorySnapshot {
        generation: 1,
        lessons: drafts
            .into_iter()
            .map(|draft| draft.into_lesson(1))
            .collect(),
    })
    .unwrap()
}

fn query(text: &str, paths: &[&str]) -> RetrievalQuery {
    RetrievalQuery {
        workspace: workspace(),
        paths: paths.iter().map(|path| (*path).to_owned()).collect(),
        text: text.into(),
        ..Default::default()
    }
}

#[test]
fn revisions_tombstones_and_immutable_snapshots() {
    let root = private_root();
    let mut store = ActiveMemoryStore::open(&root.path().join("memory.sqlite")).unwrap();
    assert_eq!(
        store.apply_batch(&[create(draft("lesson-a"))]).unwrap()[0].revision,
        1
    );
    let before = store.snapshot(&repo_query()).unwrap();
    let mut adapted = draft("lesson-a");
    adapted.title = "Recheck renamed controls".into();
    store
        .apply_batch(&[LessonMutation::Upsert {
            expected_revision: Some(1),
            draft: Box::new(adapted),
        }])
        .unwrap();
    assert_eq!(before.lessons[0].revision, 1);
    assert_eq!(before.lessons[0].title, "Check every provider selector");
    assert_eq!(
        store.snapshot(&repo_query()).unwrap().lessons[0].revision,
        2
    );
    assert!(matches!(
        store.apply_batch(&[LessonMutation::Delete {
            id: "lesson-a".into(),
            expected_revision: 1,
        }]),
        Err(ActiveMemoryError::RevisionConflict { .. })
    ));
    store
        .apply_batch(&[LessonMutation::Delete {
            id: "lesson-a".into(),
            expected_revision: 2,
        }])
        .unwrap();
    assert!(store.snapshot(&repo_query()).unwrap().lessons.is_empty());
    let tombstone = store.record("lesson-a").unwrap().unwrap();
    assert_eq!(tombstone.version.revision, 3);
    assert!(tombstone.lesson.is_none());
    assert!(matches!(
        store.apply_batch(&[create(draft("lesson-a"))]),
        Err(ActiveMemoryError::RevisionConflict {
            actual: Some(3),
            ..
        })
    ));
}

#[test]
fn retired_lessons_do_not_consume_live_lesson_capacity() {
    let root = private_root();
    let path = root.path().join("memory.sqlite");
    let mut store = ActiveMemoryStore::open(&path).unwrap();
    let mut connection = rusqlite::Connection::open(&path).unwrap();
    let transaction = connection.transaction().unwrap();
    for index in 0..=MAX_LESSONS {
        transaction.execute(
            "INSERT INTO active_memory_records(id,revision,deleted,kind) VALUES(?1,1,1,'repository')",
            [format!("retired-{index}")],
        ).unwrap();
    }
    transaction.commit().unwrap();
    store
        .apply_batch(&[create(draft("new-active-lesson"))])
        .unwrap();
    assert_eq!(store.snapshot(&repo_query()).unwrap().lessons.len(), 1);
    assert!(matches!(
        store.apply_batch(&[create(draft("retired-0"))]),
        Err(ActiveMemoryError::RevisionConflict { .. })
    ));
}

#[test]
fn conflicting_batch_rolls_back_earlier_mutations() {
    let root = private_root();
    let mut store = ActiveMemoryStore::open(&root.path().join("memory.sqlite")).unwrap();
    store.apply_batch(&[create(draft("existing"))]).unwrap();
    let result = store.apply_batch(&[
        create(draft("new-id")),
        LessonMutation::Upsert {
            expected_revision: Some(7),
            draft: Box::new(draft("existing")),
        },
    ]);
    assert!(matches!(
        result,
        Err(ActiveMemoryError::RevisionConflict { .. })
    ));
    assert!(store.record("new-id").unwrap().is_none());
    assert_eq!(store.snapshot(&repo_query()).unwrap().generation, 1);
}

#[test]
fn independent_connections_cannot_overwrite_the_same_revision() {
    let root = private_root();
    let path = root.path().join("memory.sqlite");
    let mut first = ActiveMemoryStore::open(&path).unwrap();
    first.apply_batch(&[create(draft("shared"))]).unwrap();
    let second = ActiveMemoryStore::open(&path).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [first, second]
        .into_iter()
        .enumerate()
        .map(|(number, mut store)| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut adapted = draft("shared");
                adapted.title = format!("Writer {number}");
                barrier.wait();
                store.apply_batch(&[LessonMutation::Upsert {
                    expected_revision: Some(1),
                    draft: Box::new(adapted),
                }])
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(ActiveMemoryError::RevisionConflict { .. })))
            .count(),
        1
    );
    let reopened = ActiveMemoryStore::open(&path).unwrap();
    assert_eq!(
        reopened.record("shared").unwrap().unwrap().version.revision,
        2
    );
}

#[test]
fn snapshots_enforce_repo_graph_node_and_independent_toggles() {
    let root = private_root();
    let mut store = ActiveMemoryStore::open(&root.path().join("memory.sqlite")).unwrap();
    let hash = "a".repeat(64);
    let mut other_repo = draft("repo-other");
    if let LessonScope::Repository { repository, .. } = &mut other_repo.scope {
        *repository = "example/other".into();
    }
    let graph = |id: &str, repository: Option<&str>, node: &str, hash: &str| {
        let mut lesson = draft(id);
        lesson.scope = LessonScope::Graph {
            graph_hash: hash.into(),
            node_id: node.into(),
            repository: repository.map(str::to_owned),
        };
        lesson
    };
    store
        .apply_batch(&[
            create(draft("repo-current")),
            create(other_repo),
            create(graph("graph-global", None, "worker", &hash)),
            create(graph(
                "graph-current",
                Some("example/zeroshot"),
                "worker",
                &hash,
            )),
            create(graph(
                "graph-other-repo",
                Some("example/other"),
                "worker",
                &hash,
            )),
            create(graph("graph-reviewer", None, "reviewer", &hash)),
            create(graph("graph-other-hash", None, "worker", &"b".repeat(64))),
        ])
        .unwrap();
    let mut query = repo_query();
    query.graph_hash = Some(hash);
    query.graph_lessons = true;
    query.node_id = Some("worker".into());
    let ids = |snapshot: MemorySnapshot| {
        snapshot
            .lessons
            .into_iter()
            .map(|lesson| lesson.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(store.snapshot(&query).unwrap()),
        ["graph-current", "graph-global", "repo-current"]
    );
    query.repository_lessons = false;
    assert_eq!(
        ids(store.snapshot(&query).unwrap()),
        ["graph-current", "graph-global"]
    );
    query.graph_lessons = false;
    assert!(store.snapshot(&query).unwrap().lessons.is_empty());
    assert!(
        store
            .snapshot(&ScopeQuery::default())
            .unwrap()
            .lessons
            .is_empty()
    );
}

#[test]
fn admission_rejects_catch_all_escaping_controls_and_unknown_fields() {
    for path in [
        "",
        ".",
        "..",
        "../secret",
        "/absolute",
        "src/../secret",
        "src//file",
        "**",
        "src/*",
        "C:\\file",
    ] {
        let mut lesson = draft("invalid-path");
        if let LessonScope::Repository { files, .. } = &mut lesson.scope {
            *files = vec![path.into()];
        }
        assert!(lesson.validate().is_err(), "accepted {path:?}");
    }
    let mut empty = draft("empty");
    if let LessonScope::Repository { files, .. } = &mut empty.scope {
        files.clear();
    }
    assert!(empty.validate().is_err());
    let mut controls = draft("controls");
    controls.recommendation.push('\n');
    assert!(controls.validate().is_err());
    controls.recommendation = "Use github_pat_private".into();
    assert!(controls.validate().is_err());
    let mut value = serde_json::to_value(draft("extra")).unwrap();
    value["raw_transcript"] = "must not persist".into();
    assert!(serde_json::from_value::<LessonDraft>(value).is_err());
}

#[test]
fn ranking_uses_strongest_match_and_stable_id_ties() {
    let mut file = draft("a-file");
    let mut second_file = draft("b-file");
    let mut folder = draft("folder");
    let mut identifier = draft("identifier");
    let mut keyword = draft("keyword");
    if let LessonScope::Repository { files, .. } = &mut file.scope {
        *files = vec!["src/a.rs".into()];
    }
    second_file.scope = file.scope.clone();
    if let LessonScope::Repository { files, folders, .. } = &mut folder.scope {
        files.clear();
        folders.push("src".into());
    }
    if let LessonScope::Repository {
        files, identifiers, ..
    } = &mut identifier.scope
    {
        files.clear();
        identifiers.push("configure_bedrock".into());
    }
    if let LessonScope::Repository {
        files,
        keyword_groups,
        ..
    } = &mut keyword.scope
    {
        files.clear();
        keyword_groups.push(vec!["claude".into(), "bedrock".into()]);
    }
    let index = index(vec![keyword, second_file, folder, identifier, file]);
    let result = index.retrieve(
        &query("configure_bedrock CLAUDE_CODE_USE_BEDROCK", &["src/a.rs"]),
        4096,
        &BTreeSet::new(),
    );
    assert_eq!(
        result
            .cards
            .iter()
            .map(|card| (card.version.id.as_str(), card.score))
            .collect::<Vec<_>>(),
        [
            ("a-file", 1000),
            ("b-file", 1000),
            ("folder", 800),
            ("identifier", 700),
            ("keyword", 500)
        ]
    );
    assert_eq!(result.bytes_used, result.context.len());
}

#[test]
fn any_associated_file_matches_independently() {
    let mut lesson = draft("multi-file");
    if let LessonScope::Repository { files, .. } = &mut lesson.scope {
        *files = vec!["src/first.rs".into(), "src/second.rs".into()];
    }
    let index = index(vec![lesson]);
    let result = index.retrieve(&query("", &["src/second.rs"]), 2048, &BTreeSet::new());
    assert_eq!(result.cards.len(), 1);
    assert_eq!(result.cards[0].score, 1000);
}

#[test]
fn path_normalization_handles_tool_cwd_absolute_paths_and_simple_cd() {
    let index = index(vec![draft("path")]);
    for mut query in [
        query(
            "",
            &[&workspace().join("zeroshot/src/claude.rs").to_string_lossy()],
        ),
        query("cd zeroshot && cat src/claude.rs", &[]),
        query("cat ./src/claude.rs", &[]),
    ] {
        if query.text == "cat ./src/claude.rs" {
            query.tool_cwd = Some("zeroshot".into());
        }
        assert_eq!(
            index.retrieve(&query, 2048, &BTreeSet::new()).cards.len(),
            1,
            "{query:?}"
        );
    }
    assert_eq!(
        normalize_path(&workspace(), &workspace().join("zeroshot"), "../src/a.rs"),
        Some("src/a.rs".into())
    );
    assert!(normalize_path(&workspace(), &workspace(), "../other/src/a.rs").is_none());
    assert!(
        index
            .retrieve(&query("python3 .edit.py", &[]), 2048, &BTreeSet::new())
            .cards
            .is_empty()
    );
}

#[test]
fn whole_identifiers_and_keyword_conjunctions_do_not_match_substrings() {
    let mut identifier = draft("identifier");
    if let LessonScope::Repository {
        files, identifiers, ..
    } = &mut identifier.scope
    {
        files.clear();
        identifiers.push("configure_bedrock".into());
    }
    let mut keywords = draft("keywords");
    if let LessonScope::Repository {
        files,
        keyword_groups,
        ..
    } = &mut keywords.scope
    {
        files.clear();
        keyword_groups.push(vec!["claude".into(), "bedrock".into()]);
    }
    let index = index(vec![identifier, keywords]);
    for text in [
        "preconfigure_bedrock",
        "claude unrelated",
        "notclaude bedrock",
        "claude notbedrock",
    ] {
        assert!(
            index
                .retrieve(&query(text, &[]), 4096, &BTreeSet::new())
                .cards
                .is_empty(),
            "{text}"
        );
    }
    for text in [
        "CLAUDE_CODE_USE_BEDROCK",
        "ClaudeBedrockEnvironment",
        "claude bedrock",
    ] {
        let result = index.retrieve(&query(text, &[]), 4096, &BTreeSet::new());
        assert_eq!(result.cards[0].version.id, "keywords");
    }
    assert_eq!(
        index
            .retrieve(&query("configure_bedrock", &[]), 4096, &BTreeSet::new())
            .cards[0]
            .score,
        700
    );
}

#[test]
fn budgets_include_utf8_full_cards_and_framing() {
    let mut lesson = draft("budget");
    lesson.title = "Überprüfe die Umgebung".into();
    let index = index(vec![lesson]);
    let query = query("", &["zeroshot/src/claude.rs"]);
    let complete = index.retrieve(&query, 4096, &BTreeSet::new());
    assert_eq!(complete.cards.len(), 1);
    assert_eq!(
        index.retrieve(&query, complete.bytes_used, &BTreeSet::new()),
        complete
    );
    let too_small = index.retrieve(&query, complete.bytes_used - 1, &BTreeSet::new());
    assert!(too_small.cards.is_empty());
    assert!(too_small.context.is_empty());
    assert_eq!(too_small.bytes_used, 0);
}

#[test]
fn deduplication_is_by_exact_id_and_revision() {
    let lesson = draft("revision").into_lesson(2);
    let index = MemoryIndex::new(MemorySnapshot {
        generation: 2,
        lessons: vec![lesson],
    })
    .unwrap();
    let query = query("", &["zeroshot/src/claude.rs"]);
    let older = BTreeSet::from([LessonVersion {
        id: "revision".into(),
        revision: 1,
    }]);
    assert_eq!(index.retrieve(&query, 4096, &older).cards.len(), 1);
    let current = BTreeSet::from([LessonVersion {
        id: "revision".into(),
        revision: 2,
    }]);
    assert!(index.retrieve(&query, 4096, &current).cards.is_empty());
}

#[test]
fn graph_guidance_is_separate_and_node_specific() {
    let mut lesson = draft("graph-only");
    lesson.scope = LessonScope::Graph {
        graph_hash: "a".repeat(64),
        node_id: "worker".into(),
        repository: None,
    };
    let index = index(vec![lesson]);
    assert!(
        index
            .retrieve(
                &query("worker claude bedrock", &["zeroshot/src/claude.rs"]),
                4096,
                &BTreeSet::new()
            )
            .cards
            .is_empty()
    );
    assert!(
        index
            .graph_guidance("reviewer", 4096, &BTreeSet::new())
            .cards
            .is_empty()
    );
    assert_eq!(
        index
            .graph_guidance("worker", 4096, &BTreeSet::new())
            .cards
            .len(),
        1
    );
}

#[test]
fn input_truncation_respects_utf8_and_query_limits() {
    let index = index(vec![draft("bounded")]);
    let text = format!("{}zeroshot/src/claude.rs", "ü".repeat(32_769));
    assert!(
        index
            .retrieve(&query(&text, &[]), 4096, &BTreeSet::new())
            .cards
            .is_empty()
    );
}

#[cfg(unix)]
#[test]
fn storage_rejects_symlinks_hardlinks_and_public_modes() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = private_root();
    let path = root.path().join("memory.sqlite");
    let store = ActiveMemoryStore::open(&path).unwrap();
    drop(store);
    let link = root.path().join("symlink.sqlite");
    symlink(&path, &link).unwrap();
    assert!(matches!(
        ActiveMemoryStore::open(&link),
        Err(ActiveMemoryError::UnsafeStorage)
    ));
    let hardlink = root.path().join("hardlink.sqlite");
    std::fs::hard_link(&path, &hardlink).unwrap();
    assert!(matches!(
        ActiveMemoryStore::open(&hardlink),
        Err(ActiveMemoryError::UnsafeStorage)
    ));
    std::fs::remove_file(hardlink).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        ActiveMemoryStore::open(&path),
        Err(ActiveMemoryError::UnsafeStorage)
    ));
}

#[cfg(windows)]
#[test]
fn windows_storage_uses_private_acls_and_rejects_a_public_wal() {
    use crate::execution::platform::{self, FileAccess};
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("nested").join("memory");
    let path = directory.join("lessons.sqlite3");
    let store = ActiveMemoryStore::open(&path).unwrap();
    platform::validate_private_file(&platform::open_directory(&directory).unwrap()).unwrap();
    drop(platform::private_file(&path, FileAccess::Read).unwrap());
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    let wal = std::path::PathBuf::from(wal);
    assert!(wal.is_file());
    drop(platform::private_file(&wal, FileAccess::Read).unwrap());
    assert!(
        std::process::Command::new("icacls.exe")
            .arg(&wal)
            .args(["/grant", "*S-1-1-0:R"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(ActiveMemoryStore::open(&path).is_err());
    drop(store);
}
