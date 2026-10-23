use super::*;

async fn multi_case() -> (Case, PathBuf) {
    let case = case().await;
    let workspace = case.fixture._directory.path().join("other-repository");
    std::fs::create_dir_all(workspace.join("src")).assert_value();
    std::fs::copy(
        case.workspace().join("src/provider.rs"),
        workspace.join("src/provider.rs"),
    )
    .assert_value();
    git(&workspace, &["init", "--quiet"]);
    let mut other = case.stored.clone();
    other.snapshot.run_id = RunId::new(uuid::Uuid::now_v7().to_string());
    other.admitted.title =
        openengine_cluster_protocol::RunTitle::new("Other repository software change")
            .assert_value();
    write_history(&case.state, &workspace, &other);
    (case, workspace)
}

fn assert_scope_partition(
    session: &Path,
    task: &Value,
    repository: &Value,
    field: &str,
) -> Vec<String> {
    let payload = read(&session.join(task["path"].as_str().assert_value()));
    let supplied = &payload["context"][field];
    let lessons = match field {
        "supplied_lessons" => supplied
            .as_object()
            .assert_value()
            .values()
            .collect::<Vec<_>>(),
        _ => supplied.as_array().assert_value().iter().collect(),
    };
    lessons
        .iter()
        .map(|lesson| {
            let scope_repository = &lesson["scope"]["repository"];
            assert!(
                scope_repository.is_null() || scope_repository == repository,
                "a specialist must never receive another repository's card"
            );
            lesson["id"].as_str().assert_value().to_owned()
        })
        .collect()
}

#[tokio::test]
async fn all_repositories_share_one_publication_with_partitioned_maintenance() {
    let (case, _) = multi_case().await;
    let mut shared = seed_lesson();
    shared.id = "shared-procedure".to_owned();
    shared.scope = LessonScope::Graph {
        graph_hash: crate::active_memory::runtime::authored_graph_hash(&case.stored.admitted)
            .assert_value(),
        node_id: "worker".to_owned(),
        repository: None,
    };
    case.store()
        .apply_batch(&[LessonMutation::Upsert {
            expected_revision: None,
            draft: Box::new(shared),
        }])
        .assert_value();
    let inventory = case.advance().await;
    assert_eq!(inventory["runs"].as_array().assert_value().len(), 2);
    assert_ne!(
        inventory["runs"][0]["repository"],
        inventory["runs"][1]["repository"]
    );
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let review = case.advance().await;
    assert_eq!(review["lessons_to_reconcile"], 5);
    let mut covered = std::collections::BTreeSet::new();
    for entry in review["maintenance_tasks"].as_array().assert_value() {
        for id in assert_scope_partition(
            &case.session,
            &entry["task"],
            &entry["run"]["repository"],
            "supplied_lessons",
        ) {
            assert!(
                covered.insert(id),
                "every card must be assigned exactly once across repositories"
            );
        }
    }
    assert_eq!(covered.len(), 5);
    case.maintenance(&review, "keep");
    let task = &review["maintenance_tasks"]
        .as_array()
        .assert_value()
        .last()
        .assert_value()["task"];
    let response_path = case.session.join(format!(
        "{}.response.json",
        task["path"].as_str().assert_value()
    ));
    let saved = std::fs::read(&response_path).assert_value();
    std::fs::remove_file(&response_path).assert_value();
    assert_eq!(case.advance().await["phase"], "maintenance");
    assert!(
        case.store()
            .record("shared-procedure")
            .assert_value()
            .is_some()
    );
    for lesson in read(&case.session.join("review.json"))["snapshot"]["lessons"]
        .as_array()
        .assert_value()
    {
        if lesson["id"] != "shared-procedure" {
            assert!(
                case.store()
                    .record(lesson["id"].as_str().assert_value())
                    .assert_value()
                    .is_none()
            );
        }
    }
    std::fs::write(response_path, saved).assert_value();
    let complete = case.advance().await;
    assert_eq!(complete["published_mutations"], 4);
    assert_eq!(complete["reviewed_lessons"], 5);
    assert_eq!(case.advance().await, complete);
    let next = case.fixture._directory.path().join("next-session");
    let next_inventory = initialize::init(case.state.clone(), &next, Default::default())
        .await
        .assert_value();
    for run in next_inventory["runs"].as_array().assert_value() {
        for task in run["tasks"].as_array().assert_value() {
            assert_scope_partition(&next, task, &run["repository"], "existing_lessons");
        }
    }
}

#[tokio::test]
async fn ignore_repository_aliases_persist_and_report_exclusions() {
    let (case, other) = multi_case().await;
    let other = std::fs::canonicalize(other).assert_value();
    let workspace = std::fs::canonicalize(case.workspace()).assert_value();
    let original = case.advance().await;
    let run = original["runs"]
        .as_array()
        .assert_value()
        .iter()
        .find(|run| run["workspace"] == json!(other))
        .assert_value();
    for (index, alias) in run["repository_aliases"]
        .as_array()
        .assert_value()
        .iter()
        .enumerate()
    {
        let directory = case
            .fixture
            ._directory
            .path()
            .join(format!("ignored-{index}"));
        let filters = initialize::Ignore::new(&[alias.as_str().assert_value().to_owned()], &[])
            .assert_value();
        let inventory = initialize::init(case.state.clone(), &directory, filters)
            .await
            .assert_value();
        assert_eq!(inventory["runs"].as_array().assert_value().len(), 1);
        assert_eq!(inventory["runs"][0]["workspace"], json!(workspace));
        assert!(
            inventory["excluded"][0]["reason"]
                .as_str()
                .assert_value()
                .contains("ignored")
        );
        assert_eq!(
            initialize::init(case.state.clone(), &directory, Default::default())
                .await
                .assert_value(),
            inventory
        );
    }
}

#[tokio::test]
async fn ignored_graph_name_or_hash_skips_every_matching_run_without_losing_reports() {
    let (case, _) = multi_case().await;
    let inventory = case.advance().await;
    for key in ["graph_name", "graph_hash"] {
        let directory = case.fixture._directory.path().join(key);
        let filters = initialize::Ignore::new(
            &[],
            &[inventory["runs"][0][key].as_str().assert_value().to_owned()],
        )
        .assert_value();
        let done = initialize::init(case.state.clone(), &directory, filters)
            .await
            .assert_value();
        assert_eq!(done["phase"], "complete");
        assert_eq!(done["published_mutations"], 0);
        assert_eq!(done["excluded"].as_array().assert_value().len(), 2);
        assert_eq!(
            initialize::init(case.state.clone(), &directory, Default::default())
                .await
                .assert_value(),
            done
        );
    }
}

#[tokio::test]
async fn a_change_in_another_repository_refreshes_all_maintenance_before_publication() {
    let (case, other) = multi_case().await;
    let inventory = case.advance().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let review = case.advance().await;
    case.maintenance(&review, "keep");
    std::fs::write(
        other.join("src/provider.rs"),
        "fn reject_provider_controls() { changed(); }\n",
    )
    .assert_value();
    let refreshed = case.advance().await;
    assert_eq!(refreshed["refresh_reason"], "source_or_lessons_changed");
    assert_ne!(review["source_digest"], refreshed["source_digest"]);
    assert_eq!(case.advance().await["phase"], "maintenance");
    case.maintenance(&refreshed, "keep");
    assert_eq!(case.advance().await["published_mutations"], 4);
}

#[test]
fn memory_cli_accepts_repeated_comma_and_space_separated_ignore_lists() {
    use crate::native_v2_cli::{MemoryCommand, NativeV2CliCommand, parse_native_v2_args};
    let command = parse_native_v2_args(
        [
            "memory",
            "init",
            "--directory",
            "session",
            "--ignore-repos",
            "owner/one,two",
            "three",
            "--ignore-repos",
            "four",
            "--ignore-graphs",
            "research,hash",
            "--ignore-graphs",
            "other",
        ]
        .into_iter()
        .map(std::ffi::OsString::from),
    )
    .assert_value();
    assert_eq!(
        command,
        NativeV2CliCommand::Memory(MemoryCommand::Init {
            directory: "session".into(),
            ignore_repos: ["owner/one", "two", "three", "four"]
                .map(str::to_owned)
                .to_vec(),
            ignore_graphs: ["research", "hash", "other"].map(str::to_owned).to_vec(),
        })
    );
    for entry in ["", "  ", "repo\nname"] {
        assert!(initialize::Ignore::new(&[entry.to_owned()], &[]).is_err());
    }
}

#[tokio::test]
async fn changing_only_checkout_identity_cannot_publish_into_the_old_repository() {
    for detached in [false, true] {
        assert_identity_replacement(detached).await;
    }
}

async fn assert_identity_replacement(detached: bool) {
    let case = case().await;
    git(
        &case.workspace(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "--quiet",
            "-m",
            "source",
        ],
    );
    git(
        &case.workspace(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/original.git",
        ],
    );
    if detached {
        git(&case.workspace(), &["checkout", "--quiet", "--detach"]);
    }
    let inventory = case.advance().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let review = case.advance().await;
    case.maintenance(&review, "keep");
    git(
        &case.workspace(),
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/example/replacement.git",
        ],
    );
    let result = initialize::init(case.state.clone(), &case.session, Default::default()).await;
    assert!(
        result
            .err()
            .assert_value()
            .to_string()
            .contains("repository identity changed")
    );
    for lesson in read(&case.session.join("review.json"))["snapshot"]["lessons"]
        .as_array()
        .assert_value()
    {
        assert!(
            case.store()
                .record(lesson["id"].as_str().assert_value())
                .assert_value()
                .is_none()
        );
    }
}

#[cfg(unix)]
fn linked_authority(case: &Case, ignored: bool) -> PathBuf {
    let directory = case.workspace().join("implementation");
    std::fs::create_dir(&directory).assert_value();
    let target = directory.join("provider.rs");
    std::fs::rename(case.workspace().join("src/provider.rs"), &target).assert_value();
    std::os::unix::fs::symlink(
        "../implementation/provider.rs",
        case.workspace().join("src/provider.rs"),
    )
    .assert_value();
    if ignored {
        std::fs::write(
            case.workspace().join(".git/info/exclude"),
            "implementation/\n",
        )
        .assert_value();
    }
    target
}

#[cfg(unix)]
#[tokio::test]
async fn a_tracked_symlink_cannot_hide_ignored_authority_from_the_source_fingerprint() {
    let case = case().await;
    let target = linked_authority(&case, true);
    let inventory = case.advance().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let review = case.advance().await;
    case.maintenance(&review, "keep");
    std::fs::write(target, "fn reject_provider_controls() { changed(); }\n").assert_value();
    let result = initialize::init(case.state.clone(), &case.session, Default::default()).await;
    assert!(
        result
            .err()
            .assert_value()
            .to_string()
            .contains("canonical symlink targets")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn nonignored_symlink_targets_are_fingerprinted_and_can_support_current_authority() {
    let case = case().await;
    let target = linked_authority(&case, false);
    let inventory = case.advance().await;
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let review = case.advance().await;
    case.maintenance(&review, "keep");
    std::fs::write(target, "fn reject_provider_controls() { changed(); }\n").assert_value();
    let refreshed = case.advance().await;
    assert_eq!(refreshed["refresh_reason"], "source_or_lessons_changed");
    case.maintenance(&refreshed, "keep");
    assert_eq!(case.advance().await["published_mutations"], 2);
}
