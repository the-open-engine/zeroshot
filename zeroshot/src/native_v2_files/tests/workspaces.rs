use super::*;
use openengine_cluster_testkit::assertions::AssertError;
use openengine_cluster_protocol::{RunWorkspace, WorkspacePlacement};
use crate::native_v2_platform as platform;

fn workspace(seeds: Vec<FileSelection>) -> RunWorkspace {
    RunWorkspace {
        placement: WorkspacePlacement::Managed,
        local_directory: None,
        copy_root: false,
        seeds,
    }
}
fn files(root: &Path) -> WorkspaceFiles {
    WorkspaceFiles {
        host: Arc::new(store(root)),
        target: "target-one".into(),
        identity: identity("user"),
        directory: root.join("inputs"),
    }
}
fn freeze(files: &WorkspaceFiles, spec: &RunWorkspace, run: &str) -> RunId {
    files
        .freeze(FreezeWorkspace {
            run_id: &RunId::new(run),
            submission_key: &operation("submission"),
            workspace: spec,
            local_root: None,
        })
        .assert_value()
}
fn seed(name: &str) -> FileSelection {
    FileSelection {
        address: FileAddress {
            scope: FileScope::User,
            path: path(name),
        },
        member: path(""),
    }
}
fn upload(files: &WorkspaceFiles, selection: &FileSelection, source: &Path, op: &str) {
    let mut transfer = Vec::new();
    pack(source, &mut transfer).assert_value();
    files
        .host
        .replace(
            &FileAccess {
                target: &files.target,
                identity: &files.identity,
            },
            &FileReplaceRequest {
                address: selection.address.clone(),
                operation: operation(op),
            },
            &mut Cursor::new(transfer),
        )
        .assert_value();
}

#[test]
fn run_id_conflicts_preserve_frozen_manifests_and_leave_rejected_keys_unbound() {
    for seeded in [false, true] {
        for same_inputs in [false, true] {
            let temp = tempdir();
            let files = files(temp.path());
            let source = temp.path().join("seed.txt");
            std::fs::write(&source, b"original bytes").assert_value();
            let selection = seed("library/seed.txt");
            upload(&files, &selection, &source, "upload");
            let selected = |include| {
                workspace(if include {
                    vec![selection.clone()]
                } else {
                    vec![]
                })
            };
            let spec = selected(seeded);
            let original = freeze(&files, &spec, "original-run");
            let manifests = || {
                std::fs::read_dir(&files.directory)
                    .assert_value()
                    .map(|entry| entry.assert_value().path())
                    .filter(|path| {
                        path.extension()
                            .is_some_and(|extension| extension == "json")
                    })
                    .map(|path| (path.clone(), std::fs::read(path).assert_value()))
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            let before = manifests();
            let conflicting = selected(if same_inputs { seeded } else { !seeded });
            let rejected_key = operation("rejected-key");
            let error = files
                .freeze(FreezeWorkspace {
                    run_id: &original,
                    submission_key: &rejected_key,
                    workspace: &conflicting,
                    local_root: None,
                })
                .assert_error();
            assert!(
                error
                    .to_string()
                    .contains("run ID already identifies a different submission")
            );
            assert_eq!(manifests(), before);
            assert_eq!(
                files
                    .host
                    .pin_exists(
                        &FileAccess {
                            target: &files.target,
                            identity: &files.identity,
                        },
                        &FilePin {
                            run_id: original.clone(),
                            operation: operation("workspace-seed-0"),
                        }
                    )
                    .assert_value(),
                seeded
            );
            let new_run = RunId::new("new-run");
            assert_eq!(
                files
                    .freeze(FreezeWorkspace {
                        run_id: &new_run,
                        submission_key: &rejected_key,
                        workspace: &conflicting,
                        local_root: None,
                    })
                    .assert_value(),
                new_run
            );
            // A lost-ack replay keeps its original identity even when its new proposal is occupied.
            assert_eq!(freeze(&files, &spec, "new-run"), original);
            let destination = temp.path().join("workspace");
            std::fs::create_dir(&destination).assert_value();
            files.materialize(&original, &destination).assert_value();
            if seeded {
                assert_eq!(
                    std::fs::read(destination.join("seed.txt")).assert_value(),
                    b"original bytes"
                );
            } else {
                assert_eq!(std::fs::read_dir(destination).assert_value().count(), 0);
            }
        }
    }
}

struct QueuedWorkspaceAllocation {
    files: WorkspaceFiles,
    destination: PathBuf,
    entered: tokio::sync::Notify,
    released: tokio::sync::Notify,
    materialized: tokio::sync::Notify,
}

struct QueuedClaim;
impl platform::ExclusiveControllerClaim for QueuedClaim {}

#[async_trait::async_trait]
impl platform::CapsuleAllocator for QueuedWorkspaceAllocation {
    async fn claim_controller(
        &self,
        _: &RunId,
    ) -> std::result::Result<
        Arc<dyn platform::ExclusiveControllerClaim>,
        platform::ControllerClaimUnavailable,
    > {
        Ok(Arc::new(QueuedClaim))
    }

    async fn allocate(
        &self,
        request: platform::CapsuleAllocationRequest<'_>,
    ) -> std::result::Result<platform::AllocatedCapsule, platform::CapsuleAllocationUnavailable>
    {
        self.entered.notify_one();
        self.released.notified().await;
        self.files
            .materialize(request.run_id, &self.destination)
            .assert_value();
        self.materialized.notify_one();
        std::future::pending().await
    }

    async fn destroy_or_confirm_absent(
        &self,
        _: &RunId,
        _: crate::native_v2_supervisor::RunRuntimeExit,
    ) -> std::result::Result<platform::CapsuleDestroyed, platform::CapsuleCleanupUnavailable> {
        Ok(platform::CapsuleDestroyed::confirmed())
    }
}

#[tokio::test]
async fn queued_factory_run_keeps_pinned_inputs_after_conflicting_run_id_submission() {
    use crate::native_v2_hosting::{ProductionHostingConfig, ProductionTargetControllerFactory};
    use crate::native_v2_target_authority::{TargetControllerFactory, TargetRunRequest};
    use crate::v2_run_ledger::sqlite::SqliteRunLedger;
    let temp = tempdir();
    let files = files(temp.path());
    let source = temp.path().join("seed.txt");
    std::fs::write(&source, b"original pinned bytes").assert_value();
    let selection = seed("seed.txt");
    upload(&files, &selection, &source, "upload");
    let destination = temp.path().join("workspace");
    std::fs::create_dir(&destination).assert_value();
    let allocator = Arc::new(QueuedWorkspaceAllocation {
        files: files.clone(),
        destination: destination.clone(),
        entered: tokio::sync::Notify::new(),
        released: tokio::sync::Notify::new(),
        materialized: tokio::sync::Notify::new(),
    });
    let controller = platform::NativeV2PlatformController::new_with_delivery_policy(
        Arc::new(SqliteRunLedger::open(temp.path().join("runs.sqlite3")).assert_value()),
        allocator.clone(),
        crate::native_v2_admission::DeliveryPolicy::Optional,
    )
    .await
    .assert_value();
    let factory = ProductionTargetControllerFactory::new(ProductionHostingConfig {
        workspace_files: Some(files.clone()),
        ..Default::default()
    });
    let submission: openengine_cluster_protocol::RunSubmission = serde_json::from_value(serde_json::json!({
        "title":"Queued workspace",
        "graph":{"profile":"openengine.graph.full/v1","initialInput":{"kind":"null"},
            "policy":{"policy":"policy.native-v2@1","default":"deny"},
            "root":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}},
        "initialInput":null,
        "runtime":{"harness":"codex","provider":"openai","size":"host","nodes":{},
            "activeMemory":{"repository":false,"graph":false},"globalAgents":{"postmortem":{"enabled":false}}},
        "workspace":workspace(vec![selection.clone()]), "submissionKey":"original-request"
    })).assert_value();
    let run = RunId::new("queued-run");
    let request = |submission| TargetRunRequest {
        run_id: run.clone(),
        submission,
        connections: Default::default(),
        connection_resolver: None,
        github_token: None,
        git_credentials: None,
        commit_identity: None,
        commit_co_author: None,
    };
    assert_eq!(
        factory
            .submit(&controller, request(submission.clone()))
            .await
            .assert_value()
            .run_id,
        run
    );
    let bound = std::time::Duration::from_secs(10);
    tokio::time::timeout(bound, allocator.entered.notified())
        .await
        .assert_value();
    // Admission is complete, but the original run has not materialized its inputs yet.
    let mut conflicting = submission;
    conflicting.submission_key = operation("different-request");
    conflicting.workspace = Some(workspace(vec![]));
    let error = factory
        .submit(&controller, request(conflicting))
        .await
        .assert_error();
    assert!(
        error
            .message()
            .contains("run ID already identifies a different submission")
    );
    // Changing the library too cannot replace the first submission's frozen bytes.
    std::fs::write(&source, b"replacement").assert_value();
    upload(&files, &selection, &source, "replacement");
    allocator.released.notify_one();
    tokio::time::timeout(bound, allocator.materialized.notified())
        .await
        .assert_value();
    assert_eq!(
        std::fs::read(destination.join("seed.txt")).assert_value(),
        b"original pinned bytes"
    );
    assert_eq!(std::fs::read(&source).assert_value(), b"replacement");
    controller
        .force(openengine_cluster_protocol::RunForceParams { run_id: run })
        .await
        .assert_value();
}

#[test]
fn managed_inputs_survive_lost_ack_library_replacement_deletion_and_store_restart() {
    let temp = tempdir();
    let files = files(temp.path());
    let source = temp.path().join("source");
    std::fs::create_dir_all(source.join("nested/empty")).assert_value();
    std::fs::write(source.join("one.txt"), b"original one").assert_value();
    std::fs::write(source.join("two.txt"), b"original two").assert_value();
    std::fs::write(source.join("nested/child.txt"), b"original child").assert_value();
    let seeds = vec![
        seed("library/one.txt"),
        seed("library/two.txt"),
        seed("library/nested"),
    ];
    for (index, name) in ["one.txt", "two.txt", "nested"].iter().enumerate() {
        upload(
            &files,
            &seeds[index],
            &source.join(name),
            &format!("upload-{index}"),
        );
    }
    let spec = workspace(seeds.clone());
    let accepted = freeze(&files, &spec, "original-run");
    // A response was lost: mutate every mutable source before retrying with another proposed ID.
    std::fs::write(source.join("one.txt"), b"replacement").assert_value();
    upload(&files, &seeds[0], &source.join("one.txt"), "replacement");
    files
        .host
        .remove(
            &FileAccess {
                target: &files.target,
                identity: &files.identity,
            },
            &seeds[1].address,
        )
        .assert_value();
    std::fs::remove_dir_all(&source).assert_value();
    drop(files);
    let files = self::files(temp.path());
    assert_eq!(freeze(&files, &spec, "retry-run"), accepted);
    let destination = temp.path().join("workspace");
    std::fs::create_dir(&destination).assert_value();
    files.materialize(&accepted, &destination).assert_value();
    assert_eq!(
        std::fs::read(destination.join("one.txt")).assert_value(),
        b"original one"
    );
    assert_eq!(
        std::fs::read(destination.join("two.txt")).assert_value(),
        b"original two"
    );
    assert_eq!(
        std::fs::read(destination.join("nested/child.txt")).assert_value(),
        b"original child"
    );
    assert!(destination.join("nested/empty").is_dir());
    let mut library = Vec::new();
    files
        .host
        .download(
            &FileAccess {
                target: &files.target,
                identity: &files.identity,
            },
            &seeds[0],
            &mut library,
        )
        .assert_value();
    assert!(
        library
            .windows(b"replacement".len())
            .any(|bytes| bytes == b"replacement")
    );
    // Fresh restart restores the pins, independently of the deleted source library and run edits.
    std::fs::write(destination.join("one.txt"), b"worker edit").assert_value();
    std::fs::write(destination.join("extra.txt"), b"worker extra").assert_value();
    files.restart(&accepted, &destination).assert_value();
    assert_eq!(
        std::fs::read(destination.join("one.txt")).assert_value(),
        b"original one"
    );
    assert!(!destination.join("extra.txt").exists());
}

#[test]
fn managed_local_root_copies_contents_once_without_mutating_source() {
    let temp = tempdir();
    let files = files(temp.path());
    let root = temp.path().join("original");
    std::fs::create_dir_all(root.join("empty")).assert_value();
    std::fs::write(root.join("data.txt"), b"pinned").assert_value();
    let mut spec = workspace(Vec::new());
    spec.copy_root = true;
    let run = files
        .freeze(FreezeWorkspace {
            run_id: &RunId::new("root-run"),
            submission_key: &operation("root-submission"),
            workspace: &spec,
            local_root: Some(&root),
        })
        .assert_value();
    std::fs::write(root.join("data.txt"), b"new source").assert_value();
    assert_eq!(
        files
            .freeze(FreezeWorkspace {
                run_id: &RunId::new("retry"),
                submission_key: &operation("root-submission"),
                workspace: &spec,
                local_root: Some(&root)
            })
            .assert_value(),
        run
    );
    let destination = temp.path().join("managed");
    std::fs::create_dir(&destination).assert_value();
    files.materialize(&run, &destination).assert_value();
    assert_eq!(
        std::fs::read(destination.join("data.txt")).assert_value(),
        b"pinned"
    );
    assert!(destination.join("empty").is_dir());
    assert!(!destination.join("original").exists());
    std::fs::write(destination.join("data.txt"), b"worker").assert_value();
    assert_eq!(
        std::fs::read(root.join("data.txt")).assert_value(),
        b"new source"
    );
}

#[test]
fn workspace_collisions_and_in_place_seeds_refuse_before_preparing_storage() {
    let temp = tempdir();
    let files = files(temp.path());
    let spec = workspace(vec![seed("one/Data.txt"), seed("two/data.txt")]);
    assert!(
        files
            .freeze(FreezeWorkspace {
                run_id: &RunId::new("run"),
                submission_key: &operation("key"),
                workspace: &spec,
                local_root: None
            })
            .is_err()
    );
    assert!(!files.directory.exists());
    let mut spec = workspace(vec![seed("one/data.txt")]);
    spec.placement = WorkspacePlacement::InPlace;
    assert!(spec.validate().is_err());
}

#[test]
fn copied_root_rejects_captured_external_git_administration_at_any_depth() {
    for (member, bytes) in [
        (".git", "../outside"),
        ("child/.git", "../outside"),
        (".git/commondir", "../outside"),
        ("child/.git/commondir", "../outside"),
        (".git/config", "[core]\nworktree = /original\n"),
        ("child/.git/config", "[CoRe]\nWoRkTrEe = ../..\n"),
        (".git/config.worktree", "[core]\nworktree = /original\n"),
        ("child/.git/config", "[include]\npath = /outside\n"),
        (
            ".git/config",
            "[includeIf \"gitdir:/original/\"]\npath = /outside\n",
        ),
    ] {
        let temp = tempdir();
        let files = files(temp.path());
        let root = temp.path().join("original");
        let pointer = root.join(member);
        std::fs::create_dir_all(pointer.parent().assert_value()).assert_value();
        std::fs::write(&pointer, bytes).assert_value();
        let mut spec = workspace(Vec::new());
        spec.copy_root = true;
        let error = files
            .freeze(FreezeWorkspace {
                run_id: &RunId::new("rejected-copy"),
                submission_key: &operation("copy"),
                workspace: &spec,
                local_root: Some(&root),
            })
            .assert_error();
        assert!(
            error.to_string().contains("external Git administration"),
            "{error}"
        );
        assert!(
            !files
                .host
                .pin_exists(
                    &FileAccess {
                        target: &files.target,
                        identity: &files.identity,
                    },
                    &FilePin {
                        run_id: RunId::new("rejected-copy"),
                        operation: operation("workspace-seed-0"),
                    }
                )
                .assert_value()
        );
        assert_eq!(std::fs::read(pointer).assert_value(), bytes.as_bytes());
    }
}

#[test]
fn managed_selections_validate_pinned_git_metadata_under_the_destination_basename() {
    for (member, bytes) in [
        (".git", "gitdir: /original/.git/worktrees/linked\n"),
        ("folder/.git", "gitdir: /original/.git/worktrees/linked\n"),
        ("folder/.git/commondir", "../original"),
        (".git/config", "[core]\nworktree = /original\n"),
        (
            "folder/.git/config.worktree",
            "[core]\nworktree = /original\n",
        ),
        ("folder/.git/config", "[include]\npath = /outside\n"),
    ] {
        let temp = tempdir();
        let files = files(temp.path());
        let library = temp.path().join("library-source");
        std::fs::create_dir_all(library.join(member).parent().assert_value()).assert_value();
        std::fs::write(library.join(member), bytes).assert_value();
        let mut selection = seed("library");
        upload(&files, &selection, &library, "upload");
        selection.member = path(member.split('/').next().assert_value());
        let spec = workspace(vec![selection.clone()]);
        let accept = || {
            files.freeze(FreezeWorkspace {
                run_id: &RunId::new("invalid-seed"),
                submission_key: &operation("selection"),
                workspace: &spec,
                local_root: None,
            })
        };
        let error = accept().assert_error();
        assert!(
            error.to_string().contains("external Git administration"),
            "{error}"
        );
        let access = FileAccess {
            target: &files.target,
            identity: &files.identity,
        };
        let mut pinned = Vec::new();
        files
            .host
            .read_pin(
                &access,
                &FilePin {
                    run_id: RunId::new("invalid-seed"),
                    operation: operation("workspace-seed-0"),
                },
                &mut pinned,
            )
            .assert_value();
        let mut original = Vec::new();
        files
            .host
            .download(&access, &selection, &mut original)
            .assert_value();
        assert_eq!(pinned, original);
        assert_eq!(
            std::fs::read(library.join(member)).assert_value(),
            bytes.as_bytes()
        );
        // Replacing the library cannot turn an already captured unsafe input into a safe one.
        std::fs::remove_dir_all(&library).assert_value();
        std::fs::create_dir(&library).assert_value();
        std::fs::write(library.join("safe"), "replacement").assert_value();
        upload(&files, &seed("library"), &library, "replace");
        assert!(
            accept()
                .assert_error()
                .to_string()
                .contains("external Git administration")
        );
    }
}

#[test]
fn managed_selection_keeps_self_contained_git_configuration_and_library_bytes() {
    let temp = tempdir();
    let files = files(temp.path());
    let source = temp.path().join("source");
    std::fs::create_dir_all(source.join(".git")).assert_value();
    let config = b"[core]\nrepositoryformatversion = 0\nfilemode = true\nbare = false\n";
    std::fs::write(source.join(".git/config"), config).assert_value();
    let selection = seed("repository");
    upload(&files, &selection, &source, "upload");
    let accepted = freeze(&files, &workspace(vec![selection]), "ordinary-repository");
    let destination = temp.path().join("managed");
    std::fs::create_dir(&destination).assert_value();
    files.materialize(&accepted, &destination).assert_value();
    assert_eq!(
        std::fs::read(destination.join("repository/.git/config")).assert_value(),
        config
    );
    assert_eq!(
        std::fs::read(source.join(".git/config")).assert_value(),
        config
    );
}

#[test]
fn administrative_layouts_are_validated_without_a_git_directory_name() {
    for seeded in [false, true] {
        for nested in ["", "vendor/administration"] {
            for (name, bytes) in [
                ("config", "[core]\nworktree = /original\n"),
                ("config.worktree", "[core]\nworktree = /original\n"),
                ("config", "[include]\npath = /outside\n"),
                ("commondir", "/outside"),
                ("gitdir", "/original/.git"),
            ] {
                let temp = tempdir();
                let files = files(temp.path());
                let source = temp.path().join("source");
                let admin = source.join(nested);
                std::fs::create_dir_all(admin.join("objects")).assert_value();
                std::fs::create_dir_all(admin.join("refs")).assert_value();
                std::fs::write(admin.join("HEAD"), "ref: refs/heads/main\n").assert_value();
                std::fs::write(admin.join(name), bytes).assert_value();
                let mut spec = workspace(Vec::new());
                if seeded {
                    let selection = seed("library");
                    upload(&files, &selection, &source, "upload");
                    spec.seeds.push(selection);
                } else {
                    spec.copy_root = true;
                }
                let error = files
                    .freeze(FreezeWorkspace {
                        run_id: &RunId::new("layout"),
                        submission_key: &operation("layout"),
                        workspace: &spec,
                        local_root: (!seeded).then_some(source.as_path()),
                    })
                    .assert_error();
                assert!(
                    error.to_string().contains("external Git administration"),
                    "{error}"
                );
                assert_eq!(
                    std::fs::read(admin.join(name)).assert_value(),
                    bytes.as_bytes()
                );
            }
        }
    }
}

#[test]
fn administrative_layout_assembled_from_separate_pins_is_refused() {
    let temp = tempdir();
    let files = files(temp.path());
    let source = temp.path().join("source");
    std::fs::create_dir_all(source.join("objects")).assert_value();
    std::fs::create_dir_all(source.join("refs")).assert_value();
    std::fs::write(source.join("HEAD"), "ref: refs/heads/main\n").assert_value();
    std::fs::write(source.join("config"), "[core]\nworktree = /original\n").assert_value();
    upload(&files, &seed("library"), &source, "upload");
    let spec = workspace(
        ["HEAD", "config", "objects", "refs"]
            .into_iter()
            .map(|name| {
                let mut selection = seed("library");
                selection.member = path(name);
                selection
            })
            .collect(),
    );
    let error = files
        .freeze(FreezeWorkspace {
            run_id: &RunId::new("assembled"),
            submission_key: &operation("assembled"),
            workspace: &spec,
            local_root: None,
        })
        .assert_error();
    assert!(
        error.to_string().contains("external Git administration"),
        "{error}"
    );
}

#[test]
fn commondir_layout_needs_no_local_object_store_and_plain_config_is_preserved() {
    for external_common in [false, true] {
        let temp = tempdir();
        let files = files(temp.path());
        let source = temp.path().join("source");
        std::fs::create_dir_all(&source).assert_value();
        std::fs::write(source.join("HEAD"), "ref: refs/heads/main\n").assert_value();
        let config = "[core]\nworktree = application data\n";
        std::fs::write(source.join("config"), config).assert_value();
        if external_common {
            std::fs::write(source.join("commondir"), "/outside").assert_value();
        }
        let selection = seed("library");
        upload(&files, &selection, &source, "upload");
        let result = files.freeze(FreezeWorkspace {
            run_id: &RunId::new("metadata"),
            submission_key: &operation("metadata"),
            workspace: &workspace(vec![selection]),
            local_root: None,
        });
        if external_common {
            assert!(
                result
                    .assert_error()
                    .to_string()
                    .contains("external Git administration")
            );
        } else {
            let accepted = result.assert_value();
            let destination = temp.path().join("managed");
            std::fs::create_dir(&destination).assert_value();
            files.materialize(&accepted, &destination).assert_value();
            assert_eq!(
                std::fs::read_to_string(destination.join("library/config")).assert_value(),
                config
            );
        }
        assert_eq!(
            std::fs::read_to_string(source.join("config")).assert_value(),
            config
        );
    }
}
