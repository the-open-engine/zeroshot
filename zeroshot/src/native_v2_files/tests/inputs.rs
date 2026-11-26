use super::*;
use super::workspaces::{files, seed, upload};
use crate::native_v2_files::inputs::InputAdmission;
use openengine_cluster_protocol::{PayloadType, RunFileInput};
use serde_json::{Value, json};

fn schema() -> PayloadType {
    serde_json::from_value(json!({"kind":"record","fields":{
        "left":{"required":true,"type":{"kind":"filepath"}},
        "right":{"required":true,"type":{"kind":"filepath"}},
        "folder":{"required":false,"type":{"kind":"filepath"}}
    }}))
    .assert_value()
}
fn admit(
    files: &WorkspaceFiles,
    value: &Value,
    identity: (&str, &str, &str),
) -> io::Result<(RunId, Value)> {
    let (run, owner, key) = identity;
    files
        .freeze_file_inputs(InputAdmission {
            run_id: &RunId::new(run),
            owner: &RunId::new(owner),
            key: &operation(key),
            schema: &schema(),
            value,
            local_root: None,
        })
        .map(|frozen| (frozen.run_id, frozen.value))
}
fn staged(files: &WorkspaceFiles, source: &Path, op: &str) -> Value {
    let pin = FilePin {
        run_id: RunId::new("private-upload"),
        operation: operation(op),
    };
    let mut transfer = Vec::new();
    pack(source, &mut transfer).assert_value();
    files
        .host
        .stage_upload(
            &FileAccess {
                target: &files.target,
                identity: &files.identity,
            },
            &pin,
            &mut Cursor::new(transfer),
        )
        .assert_value();
    serde_json::to_value(RunFileInput::Staged {
        run_id: pin.run_id,
        operation: pin.operation,
    })
    .assert_value()
}
fn read(value: &Value, field: &str) -> String {
    std::fs::read_to_string(value[field].as_str().unwrap()).assert_value()
}

#[test]
fn parent_relative_local_inputs_freeze_before_sources_disappear() {
    let root = tempdir();
    let files = files(root.path());
    let invoking = root.path().join("invoking");
    let source = root.path().join("dataset");
    std::fs::create_dir(&invoking).unwrap();
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("data.csv"), "accepted bytes").unwrap();
    let value = json!({"left":"../dataset/data.csv", "right":"../dataset"});
    let freeze = || {
        files.freeze_file_inputs(InputAdmission {
            run_id: &RunId::new("run"),
            owner: &RunId::new("run"),
            key: &operation("key"),
            schema: &schema(),
            value: &value,
            local_root: Some(&invoking),
        })
    };
    let accepted = freeze().unwrap().value;
    std::fs::remove_dir_all(source).unwrap();
    assert_eq!(freeze().unwrap().value, accepted);
    files
        .materialize_file_inputs(&RunId::new("run"), false)
        .unwrap();
    assert_eq!(read(&accepted, "left"), "accepted bytes");
    assert_eq!(
        std::fs::read(Path::new(accepted["right"].as_str().unwrap()).join("data.csv")).unwrap(),
        b"accepted bytes"
    );
    files.release_file_inputs(&RunId::new("run")).unwrap();
}

#[cfg(unix)]
#[test]
fn parent_relative_local_inputs_do_not_hide_link_traversal() {
    let root = tempdir();
    let files = files(root.path());
    let invoking = root.path().join("invoking");
    std::fs::create_dir(&invoking).unwrap();
    std::fs::write(root.path().join("data.csv"), "private bytes").unwrap();
    std::os::unix::fs::symlink(&invoking, invoking.join("alias")).unwrap();
    let result = files.freeze_file_inputs(InputAdmission {
        run_id: &RunId::new("run"),
        owner: &RunId::new("run"),
        key: &operation("key"),
        schema: &PayloadType::Filepath,
        value: &json!("alias/../../data.csv"),
        local_root: Some(&invoking),
    });
    assert!(result.is_err());
}

#[cfg(unix)]
#[test]
fn unreadable_input_directories_can_be_restarted_and_released() {
    use std::os::unix::fs::PermissionsExt;
    for mode in [0o000, 0o300] {
        let root = tempdir();
        let files = files(root.path());
        let source = root.path().join("dataset");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("nested/data"), "accepted bytes").unwrap();
        let original = std::fs::metadata(&source).unwrap().permissions();
        let value = json!({"left":staged(&files, &source, "left"), "right":staged(&files, &source, "right")});
        let (run, input) = admit(&files, &value, ("run", "run", "key")).unwrap();
        files.release_staged_inputs(&schema(), &value).unwrap();
        files.materialize_file_inputs(&run, false).unwrap();
        let authority = files.file_input_authority_roots(&run).unwrap();
        for restart in [true, false] {
            let copy = Path::new(input["left"].as_str().unwrap());
            let key_root = copy.parent().unwrap();
            // Repair must reach a sealed key root, a sealed selected folder and its child.
            for directory in [copy.join("nested"), copy.to_owned(), key_root.to_owned()] {
                std::fs::set_permissions(directory, std::fs::Permissions::from_mode(mode)).unwrap();
            }
            let recorded_authority = files.file_input_authority_roots(&run);
            let result = if restart {
                files.materialize_file_inputs(&run, true).map(|_| ())
            } else {
                files.release_file_inputs(&run)
            };
            // Keep a failing fixture disposable, including under a non-root account.
            if result.is_err() || recorded_authority.is_err() {
                for directory in [key_root.to_owned(), copy.to_owned(), copy.join("nested")] {
                    let _ =
                        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700));
                }
            }
            assert_eq!(recorded_authority.unwrap(), authority);
            result.unwrap();
            if restart {
                assert_eq!(
                    std::fs::read(copy.join("nested/data")).unwrap(),
                    b"accepted bytes"
                );
            }
        }
        assert!(!Path::new(input["left"].as_str().unwrap()).exists());
        assert_eq!(std::fs::metadata(&source).unwrap().permissions(), original);
        assert_eq!(
            std::fs::read(source.join("nested/data")).unwrap(),
            b"accepted bytes"
        );
    }
}

#[test]
fn cleanup_refuses_redirected_input_ancestors_and_preserves_external_files() {
    for depth in [0, 1, 2] {
        let root = tempdir();
        let files = files(root.path());
        let source = root.path().join("data.csv");
        std::fs::write(&source, "accepted bytes").unwrap();
        let value = json!({"left":staged(&files, &source, "upload"),
            "right":staged(&files, &source, "other-upload")});
        let (run, input) = admit(&files, &value, ("run", "run", "key")).unwrap();
        files.release_staged_inputs(&schema(), &value).unwrap();
        files.materialize_file_inputs(&run, false).unwrap();
        let key_root = Path::new(input["left"].as_str().unwrap()).parent().unwrap();
        let ancestor = key_root.ancestors().nth(depth.max(1)).unwrap();
        let outside = root.path().join("outside");
        let sentinel = outside
            .join(key_root.strip_prefix(ancestor).unwrap())
            .join("sentinel");
        std::fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
        std::fs::write(&sentinel, "unrelated user file").unwrap();
        if depth != 0 {
            // Remove the owned tree before installing the redirect. A full-path rename of its
            // parent would conflict with the live file store's ancestor guards on Windows.
            std::fs::remove_dir_all(ancestor).unwrap();
            #[cfg(unix)]
            std::os::unix::fs::symlink(&outside, ancestor).unwrap();
            #[cfg(windows)]
            {
                let result = std::process::Command::new("cmd")
                    .args(["/C", "mklink", "/J"])
                    .arg(ancestor)
                    .arg(&outside)
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "{}",
                    String::from_utf8_lossy(&result.stderr)
                );
            }
        }
        let result = files.release_file_inputs(&run);
        assert_eq!(result.is_ok(), depth == 0, "depth {depth}: {result:?}");
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"unrelated user file");
        if depth == 0 {
            assert!(!key_root.exists());
        }
    }
}

#[test]
fn uploads_with_equal_basenames_and_a_directory_freeze_before_queue_and_retry() {
    let root = tempdir();
    let files = files(root.path());
    let left = root.path().join("left");
    let right = root.path().join("right");
    std::fs::create_dir(&left).assert_value();
    std::fs::create_dir(&right).assert_value();
    std::fs::write(left.join("data.csv"), "accepted left").assert_value();
    std::fs::write(right.join("data.csv"), "accepted right").assert_value();
    let input = json!({"left":staged(&files, &left.join("data.csv"), "left"),
        "right":staged(&files, &right.join("data.csv"), "right"),
        "folder":staged(&files, &left, "folder")});
    let (run, accepted) = admit(&files, &input, ("original", "original", "key")).assert_value();
    files
        .release_staged_inputs(&schema(), &input)
        .assert_value();
    std::fs::remove_dir_all(left).assert_value();
    std::fs::write(right.join("data.csv"), "changed original").assert_value();
    let replay = admit(&files, &input, ("retry", "retry", "key")).assert_value();
    assert_eq!(replay, (run.clone(), accepted.clone()));
    files.materialize_file_inputs(&run, false).assert_value();
    assert_ne!(accepted["left"], accepted["right"]);
    assert_eq!(read(&accepted, "left"), "accepted left");
    assert_eq!(read(&accepted, "right"), "accepted right");
    assert_eq!(
        std::fs::read_to_string(Path::new(accepted["folder"].as_str().unwrap()).join("data.csv"))
            .assert_value(),
        "accepted left"
    );
    std::fs::write(accepted["left"].as_str().unwrap(), "node edit").assert_value();
    files.materialize_file_inputs(&run, false).assert_value();
    assert_eq!(read(&accepted, "left"), "node edit");
    files.materialize_file_inputs(&run, true).assert_value();
    assert_eq!(read(&accepted, "left"), "accepted left");
    files.release_file_inputs(&run).assert_value();
    assert!(!Path::new(accepted["left"].as_str().unwrap()).exists());
    assert_eq!(
        std::fs::read_to_string(right.join("data.csv")).assert_value(),
        "changed original"
    );
}

#[test]
fn required_fields_authority_and_both_admission_identities_precede_pin_mutations() {
    let root = tempdir();
    let files = files(root.path());
    let source = root.path().join("data.csv");
    std::fs::write(&source, "frozen").assert_value();
    upload(&files, &seed("library/data"), &source, "library");
    let selected = serde_json::to_value(RunFileInput::Target {
        selection: seed("library/data"),
    })
    .assert_value();
    assert!(
        admit(
            &files,
            &json!({"left":selected}),
            ("missing", "missing", "missing")
        )
        .is_err()
    );
    let value = json!({"left":selected,"right":selected});
    let (_, accepted) = admit(&files, &value, ("first", "first", "first-key")).assert_value();
    assert!(admit(&files, &value, ("first", "first", "different-key")).is_err());
    let mut unauthorized = files.clone();
    unauthorized.identity = identity("other-user");
    assert!(
        admit(
            &unauthorized,
            &value,
            ("unauthorized", "unauthorized", "unauthorized")
        )
        .is_err()
    );
    std::fs::write(&source, "replacement").assert_value();
    upload(&files, &seed("library/data"), &source, "replace");
    files
        .materialize_file_inputs(&RunId::new("first"), false)
        .assert_value();
    assert_eq!(read(&accepted, "left"), "frozen");
    assert!(admit(&files, &value, ("fresh-run", "fresh-run", "different-key")).is_ok());
    let changed = json!({"left":serde_json::to_value(RunFileInput::Target { selection: seed("elsewhere") }).assert_value(),"right":selected});
    assert!(admit(&files, &changed, ("other", "other", "first-key")).is_err());
}

#[test]
fn in_place_inputs_and_successor_answers_are_private_and_release_without_touching_live_work() {
    let root = tempdir();
    let files = files(root.path());
    let workspace = root.path().join("users-folder");
    std::fs::create_dir(&workspace).assert_value();
    std::fs::write(workspace.join("data.csv"), "accepted input").assert_value();
    let original = json!({"left":"data.csv","right":"data.csv"});
    let frozen = files
        .freeze_file_inputs(InputAdmission {
            run_id: &RunId::new("run"),
            owner: &RunId::new("run"),
            key: &operation("submit"),
            schema: &schema(),
            value: &original,
            local_root: Some(&workspace),
        })
        .assert_value();
    let input = frozen.value;
    std::fs::write(workspace.join("data.csv"), "accepted answer").assert_value();
    let frozen = files
        .freeze_file_inputs(InputAdmission {
            run_id: &RunId::new("answer"),
            owner: &RunId::new("run"),
            key: &operation("answer"),
            schema: &schema(),
            value: &original,
            local_root: Some(&workspace),
        })
        .assert_value();
    let answer = frozen.value;
    std::fs::write(workspace.join("data.csv"), "user edit").assert_value();
    std::fs::write(workspace.join("output.txt"), "live output").assert_value();
    files
        .materialize_file_inputs(&RunId::new("run"), false)
        .assert_value();
    assert!(!Path::new(input["left"].as_str().unwrap()).starts_with(&workspace));
    assert_eq!(read(&input, "left"), "accepted input");
    assert_eq!(read(&answer, "left"), "accepted answer");
    files
        .materialize_file_inputs(&RunId::new("run"), true)
        .assert_value();
    assert_eq!(read(&answer, "left"), "accepted answer");
    files
        .release_file_input_part(&RunId::new("abandoned"))
        .assert_value();
    assert_eq!(read(&input, "left"), "accepted input");
    files.release_file_inputs(&RunId::new("run")).assert_value();
    assert!(!Path::new(answer["left"].as_str().unwrap()).exists());
    assert_eq!(
        std::fs::read_to_string(workspace.join("data.csv")).assert_value(),
        "user edit"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("output.txt")).assert_value(),
        "live output"
    );
    assert!(
        files
            .materialize_file_inputs(&RunId::new("run"), true)
            .assert_value()
            .is_empty()
    );
}

#[test]
fn accepted_published_folder_survives_its_source_retention_boundary() {
    use crate::native_v2_files::publication::FilePublication;
    use openengine_cluster_protocol::{FieldName, PublishedFile, UnixTimestampMillis};
    let root = tempdir();
    let disk = store(root.path());
    let files = WorkspaceFiles {
        host: Arc::new(disk.clone()),
        identity: identity("user"),
        target: "target-one".into(),
        directory: root.path().join("inputs"),
    };
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(workspace.join("folder")).assert_value();
    std::fs::write(workspace.join("folder/data.csv"), "published bytes").assert_value();
    let access = FileAccess {
        target: &files.target,
        identity: &files.identity,
    };
    let published = disk
        .capture_publication(
            &access,
            &workspace,
            &FilePublication {
                owner: FilePublicationOwner::Final {
                    run_id: RunId::new("publisher"),
                },
                paths: [(FieldName::new("folder").assert_value(), "folder".into())].into(),
                now: UnixTimestampMillis::new(1).assert_value(),
                cancellation: crate::execution::driver::DriverCancellation::new(
                    tokio::sync::watch::channel(false).1,
                ),
                preflight: Arc::new(|_| Ok(())),
            },
        )
        .assert_value();
    let PublishedFile::Managed { address, .. } = published.values().next().unwrap() else {
        unreachable!()
    };
    let selected = serde_json::to_value(RunFileInput::Target {
        selection: FileSelection {
            address: address.clone(),
            member: path(""),
        },
    })
    .assert_value();
    let (_, accepted) = admit(
        &files,
        &json!({"left":selected,"right":selected}),
        ("day-59", "day-59", "day-59"),
    )
    .assert_value();
    let now = UnixTimestampMillis::new(61 * 24 * 60 * 60 * 1000).assert_value();
    let candidates = disk.select_publications(&access, now, false).assert_value();
    assert_eq!(
        disk.cleanup_publications(
            &access,
            &FileRetention {
                candidates: &candidates,
                now,
                all: false,
                protected: &Default::default()
            }
        )
        .assert_value(),
        1
    );
    assert!(
        disk.list_members(
            &access,
            &FileSelection {
                address: address.clone(),
                member: path("")
            }
        )
        .is_err()
    );
    files
        .materialize_file_inputs(&RunId::new("day-59"), false)
        .assert_value();
    let accepted_file = Path::new(accepted["left"].as_str().unwrap()).join("data.csv");
    assert_eq!(
        std::fs::read_to_string(&accepted_file).assert_value(),
        "published bytes"
    );
    std::fs::write(&accepted_file, "edited during run").assert_value();
    files
        .materialize_file_inputs(&RunId::new("day-59"), true)
        .assert_value();
    assert_eq!(
        std::fs::read_to_string(&accepted_file).assert_value(),
        "published bytes"
    );
}

#[test]
fn scalar_values_keep_validation_and_no_file_manifest() {
    let root = tempdir();
    let files = files(root.path());
    let scalar: PayloadType = serde_json::from_value(
        json!({"kind":"record","fields":{"text":{"required":true,"type":{"kind":"string"}}}}),
    )
    .assert_value();
    let frozen = files
        .freeze_file_inputs(InputAdmission {
            run_id: &RunId::new("scalar"),
            owner: &RunId::new("scalar"),
            key: &operation("scalar"),
            schema: &scalar,
            value: &json!({"text":"ordinary"}),
            local_root: None,
        })
        .assert_value();
    assert_eq!(frozen.run_id, RunId::new("scalar"));
    assert!(frozen.claim.is_none());
    assert_eq!(frozen.value, json!({"text":"ordinary"}));
    assert!(!files.directory.join("file-inputs").exists());
    assert!(crate::native_v2_files::inputs::validate_input(&scalar, &json!({})).is_err());
    assert!(crate::native_v2_files::inputs::validate_input(&scalar, &json!({"text":3})).is_err());
}

#[test]
fn relative_storage_exports_absolute_submission_and_answer_paths() {
    // Keep a relative spelling without changing the test process's shared current directory.
    let root = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let mut files = files(root.path());
    files.directory = PathBuf::from(root.path().file_name().unwrap()).join("inputs");
    let source = root.path().join("data.csv");
    std::fs::write(&source, "accepted bytes").unwrap();
    let upload = staged(&files, &source, "upload");
    let (run, input) = admit(
        &files,
        &json!({"left":upload,"right":upload}),
        ("run", "run", "key"),
    )
    .unwrap();
    let answer = files
        .freeze_answer_inputs(InputAdmission {
            run_id: &RunId::new("answer"),
            owner: &run,
            key: &operation("answer"),
            schema: &PayloadType::Filepath,
            value: &upload,
            local_root: None,
        })
        .unwrap();
    assert!(files.input_location(&run).unwrap().is_absolute());
    for path in [
        input["left"].as_str().unwrap(),
        input["right"].as_str().unwrap(),
        answer.value.as_str().unwrap(),
    ] {
        assert!(Path::new(path).is_absolute(), "{path}");
    }
    assert!(
        files.directory.is_relative(),
        "controller-state spelling stays unchanged"
    );
    files.materialize_file_inputs(&run, false).unwrap();
    assert_eq!(read(&input, "left"), "accepted bytes");
    assert_eq!(
        std::fs::read(answer.value.as_str().unwrap()).unwrap(),
        b"accepted bytes"
    );
    files.release_file_inputs(&run).unwrap();
}

#[test]
fn readonly_input_copies_can_be_restarted_and_released_without_changing_originals() {
    let root = tempdir();
    let files = files(root.path());
    let source = root.path().join("dataset");
    std::fs::create_dir_all(source.join("nested")).unwrap();
    std::fs::write(source.join("nested/data"), "accepted bytes").unwrap();
    let original_directory_permissions = std::fs::metadata(&source).unwrap().permissions();
    let original_file_permissions = std::fs::metadata(source.join("nested/data"))
        .unwrap()
        .permissions();
    let value =
        json!({"left":staged(&files, &source, "left"),"right":staged(&files, &source, "right")});
    let (run, input) = admit(&files, &value, ("run", "run", "key")).unwrap();
    files.release_staged_inputs(&schema(), &value).unwrap();
    files.materialize_file_inputs(&run, false).unwrap();
    for restart in [true, false] {
        for field in ["left", "right"] {
            let copy = Path::new(input[field].as_str().unwrap());
            for entry in [
                copy.join("nested/data"),
                copy.join("nested"),
                copy.to_owned(),
            ] {
                let mut permissions = std::fs::metadata(&entry).unwrap().permissions();
                permissions.set_readonly(true);
                std::fs::set_permissions(entry, permissions).unwrap();
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(
                &source,
                copy.parent().unwrap().join(format!("external-{field}")),
            )
            .unwrap_or_else(|error| assert_eq!(error.kind(), io::ErrorKind::AlreadyExists));
        }
        if restart {
            files.materialize_file_inputs(&run, true).unwrap();
            assert_eq!(
                std::fs::read(Path::new(input["left"].as_str().unwrap()).join("nested/data"))
                    .unwrap(),
                b"accepted bytes"
            );
        } else {
            files.release_file_inputs(&run).unwrap();
        }
    }
    assert!(!Path::new(input["left"].as_str().unwrap()).exists());
    assert_eq!(
        std::fs::read(source.join("nested/data")).unwrap(),
        b"accepted bytes"
    );
    assert_eq!(
        std::fs::metadata(&source).unwrap().permissions(),
        original_directory_permissions
    );
    assert_eq!(
        std::fs::metadata(source.join("nested/data"))
            .unwrap()
            .permissions(),
        original_file_permissions
    );
}

#[cfg(windows)]
#[test]
fn readonly_input_alias_cannot_change_external_file_permissions() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, FILE_WRITE_ATTRIBUTES};
    let root = tempdir();
    let files = files(root.path());
    let source = root.path().join("original");
    std::fs::write(&source, "unrelated original").unwrap();
    let value =
        json!({"left":staged(&files, &source, "left"),"right":staged(&files, &source, "right")});
    let (run, input) = admit(&files, &value, ("run", "run", "key")).unwrap();
    files.materialize_file_inputs(&run, false).unwrap();
    let alias = Path::new(input["left"].as_str().unwrap())
        .parent()
        .unwrap()
        .join("alias");
    std::fs::hard_link(&source, &alias).unwrap();
    let original_permissions = std::fs::metadata(&source).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    std::fs::set_permissions(&source, permissions).unwrap();
    assert!(files.release_file_inputs(&run).is_err());
    assert!(std::fs::metadata(&source).unwrap().permissions().readonly());
    // Restore the fixture's own original only after proving cleanup did not alter it.
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
        .open(&source)
        .unwrap();
    file.set_permissions(original_permissions).unwrap();
    drop(file);
    files.release_file_inputs(&run).unwrap();
}

#[test]
fn answer_replay_requires_its_successor_and_lineage_but_submission_retry_keeps_first_owner() {
    let root = tempdir();
    let files = files(root.path());
    let original = root.path().join("data.csv");
    std::fs::write(&original, "accepted bytes").unwrap();
    let value = json!(original);
    let run = RunId::new("successor");
    let owner = RunId::new("source");
    let key = operation("answer");
    let request = || InputAdmission {
        run_id: &run,
        owner: &owner,
        key: &key,
        schema: &PayloadType::Filepath,
        value: &value,
        local_root: Some(root.path()),
    };
    let first = files.freeze_answer_inputs(request()).unwrap();
    std::fs::remove_file(original).unwrap();
    for (run, owner) in [("successor", "other-source"), ("other-successor", "source")] {
        let error = files
            .freeze_answer_inputs(InputAdmission {
                run_id: &RunId::new(run),
                owner: &RunId::new(owner),
                ..request()
            })
            .unwrap_err();
        assert!(error.to_string().contains("another successor or lineage"));
    }
    let replay = files.freeze_answer_inputs(request()).unwrap();
    assert_eq!(replay.value, first.value);
    assert!(replay.claim.is_none());
    let retry = files
        .freeze_file_inputs(InputAdmission {
            run_id: &RunId::new("fresh-proposal"),
            owner: &RunId::new("fresh-proposal"),
            ..request()
        })
        .unwrap();
    assert_eq!(retry.run_id, run);
    assert_eq!(retry.value, first.value);
    files.materialize_file_inputs(&owner, true).unwrap();
    assert_eq!(
        std::fs::read(first.value.as_str().unwrap()).unwrap(),
        b"accepted bytes"
    );
    assert!(
        files
            .file_input_roots(&RunId::new("other-source"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn input_authority_records_canonical_roots_without_accepting_redirected_ancestry() {
    let root = tempdir();
    let files = files(root.path());
    let source = root.path().join("data.csv");
    std::fs::write(&source, "accepted bytes").unwrap();
    let input =
        json!({"left":staged(&files, &source, "left"),"right":staged(&files, &source, "right")});
    let (run, projected) = admit(&files, &input, ("run", "run", "key")).unwrap();
    let authority = files.file_input_authority_roots(&run).unwrap();
    assert_eq!(
        Path::new(projected["left"].as_str().unwrap()).parent(),
        Some(authority[0].as_path()),
        "projected paths and execution authority must use the same spelling"
    );
    let roots = files.materialize_file_inputs(&run, false).unwrap();
    assert_eq!(authority, vec![roots[0].canonicalize().unwrap()]);
    let recorded_directory = files.directory.clone();
    assert_eq!(
        files.file_input_authority_roots(&run).unwrap(),
        vec![roots[0].canonicalize().unwrap()]
    );
    assert_eq!(files.directory, recorded_directory);
    let ancestor = roots[0].parent().unwrap();
    let outside = root.path().join("outside");
    let redirected = outside.join(roots[0].file_name().unwrap());
    std::fs::create_dir_all(&redirected).unwrap();
    for entry in std::fs::read_dir(&roots[0]).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), redirected.join(entry.file_name())).unwrap();
    }
    std::fs::remove_dir_all(ancestor).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, ancestor).unwrap();
    #[cfg(windows)]
    assert!(
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(ancestor)
            .arg(&outside)
            .status()
            .unwrap()
            .success()
    );
    assert!(files.file_input_authority_roots(&run).is_err());
}

#[test]
fn damaged_input_manifests_block_replay_and_cleanup_without_losing_accepted_bytes() {
    let root = tempdir();
    let files = files(root.path());
    let source = root.path().join("data.csv");
    std::fs::write(&source, "accepted bytes").unwrap();
    let selected = staged(&files, &source, "upload");
    let value = json!({"left":selected,"right":selected});
    let (run, projected) = admit(&files, &value, ("run", "run", "key")).unwrap();
    files.materialize_file_inputs(&run, false).unwrap();
    let manifests = files.directory.join("file-inputs");
    let entries = std::fs::read_dir(&manifests)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    let find = |prefix: &str| {
        entries
            .iter()
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(prefix)
            })
            .unwrap()
            .clone()
    };
    let run_manifest = find("run-");
    let submission_manifest = find("submission-");

    for manifest in [&submission_manifest, &run_manifest] {
        let original = std::fs::read(manifest).unwrap();
        std::fs::write(manifest, b"{damaged manifest").unwrap();
        assert!(admit(&files, &value, ("retry", "run", "key")).is_err());
        if manifest == &run_manifest {
            assert!(files.file_input_roots(&run).is_err());
            assert!(files.file_input_authority_roots(&run).is_err());
            assert!(files.materialize_file_inputs(&run, true).is_err());
            assert!(files.release_file_input_part(&run).is_err());
            assert!(files.release_file_inputs(&run).is_err());
        }
        assert_eq!(read(&projected, "left"), "accepted bytes");
        std::fs::write(manifest, original).unwrap();
    }
    files.materialize_file_inputs(&run, true).unwrap();
    assert_eq!(read(&projected, "left"), "accepted bytes");
    files.release_file_inputs(&run).unwrap();
    assert!(!Path::new(projected["left"].as_str().unwrap()).exists());
}

#[test]
fn rollback_claim_belongs_only_to_the_attempt_that_created_the_manifest() {
    let root = tempdir();
    let files = files(root.path());
    let original = root.path().join("data");
    std::fs::write(&original, "accepted bytes").unwrap();
    let value = json!(original);
    let run = RunId::new("answer");
    let owner = RunId::new("source");
    let key = operation("answer");
    let request = || InputAdmission {
        run_id: &run,
        owner: &owner,
        key: &key,
        schema: &PayloadType::Filepath,
        value: &value,
        local_root: Some(root.path()),
    };
    let first = files.freeze_file_inputs(request()).unwrap();
    let replay = files.freeze_file_inputs(request()).unwrap();
    assert_eq!(first.value, replay.value);
    assert!(
        replay.claim.is_none(),
        "reusing a manifest grants no rollback authority"
    );
    let other = files
        .freeze_file_inputs(InputAdmission {
            run_id: &RunId::new("other"),
            key: &operation("other"),
            ..request()
        })
        .unwrap();
    files.materialize_file_inputs(&owner, true).unwrap();
    files
        .release_file_input_claim(&first.claim.unwrap())
        .unwrap();
    assert!(!Path::new(first.value.as_str().unwrap()).exists());
    assert_eq!(
        std::fs::read(other.value.as_str().unwrap()).unwrap(),
        b"accepted bytes"
    );
    assert_eq!(files.file_input_roots(&owner).unwrap().len(), 1);
}

#[test]
fn staged_upload_cleanup_follows_filepath_schema_and_preserves_scalar_lookalikes() {
    let root = tempdir();
    let files = files(root.path());
    let source = root.path().join("data");
    std::fs::write(&source, "private upload bytes").unwrap();
    let scalar: PayloadType = serde_json::from_value(json!({"kind":"record","fields":{
        "kind":{"required":true,"type":{"kind":"string"}},
        "runId":{"required":true,"type":{"kind":"string"}},
        "operation":{"required":true,"type":{"kind":"string"}}
    }}))
    .unwrap();
    let preserved = staged(&files, &source, "preserved");
    let selected = staged(&files, &source, "selected");
    let readable = |operation_name| {
        files
            .host
            .read_pin(
                &FileAccess {
                    target: &files.target,
                    identity: &files.identity,
                },
                &FilePin {
                    run_id: RunId::new("private-upload"),
                    operation: operation(operation_name),
                },
                &mut Vec::new(),
            )
            .is_ok()
    };
    files.release_staged_inputs(&scalar, &preserved).unwrap();
    assert!(readable("preserved"));
    let mixed: PayloadType = serde_json::from_value(json!({"kind":"record","fields":{
        "metadata":{"required":true,"type":scalar},
        "items":{"required":true,"type":{"kind":"array","items":{"kind":"record","fields":{
            "file":{"required":false,"type":{"kind":"filepath"}},
            "metadata":{"required":true,"type":scalar}
        }}}},
        "missing":{"required":true,"type":{"kind":"filepath"}}
    }}))
    .unwrap();
    // Even a refused value reclaims declared uploads without interpreting ordinary records,
    // unknown fields, or objects nested inside a malformed filepath selection as authority.
    files
        .release_staged_inputs(
            &mixed,
            &json!({
                "metadata":preserved,
                "items":[{"file":selected,"metadata":preserved},
                         {"file":{"nested":preserved},"metadata":preserved}],
                "undeclared":preserved
            }),
        )
        .unwrap();
    assert!(!readable("selected"));
    assert!(readable("preserved"));
}

#[test]
fn abandoned_upload_cleanup_never_releases_accepted_or_other_private_pins() {
    let root = tempdir();
    let files = files(root.path());
    let original = root.path().join("data.csv");
    std::fs::write(&original, "accepted bytes").unwrap();
    let upload = staged(&files, &original, "upload");
    let (_, accepted) = admit(
        &files,
        &json!({"left":upload,"right":upload}),
        ("run", "run", "inputs"),
    )
    .unwrap();
    let access = FileAccess {
        target: &files.target,
        identity: &files.identity,
    };
    let unrelated = FilePin {
        run_id: RunId::new("other"),
        operation: operation("private"),
    };
    let mut transfer = Vec::new();
    pack(&original, &mut transfer).unwrap();
    files
        .host
        .stage(&access, &unrelated, &mut Cursor::new(transfer))
        .unwrap();
    files
        .release_staged_inputs(
            &PayloadType::Filepath,
            &json!({"kind":"staged","runId":unrelated.run_id,"operation":unrelated.operation}),
        )
        .unwrap();
    assert!(files.host.pin_exists(&access, &unrelated).unwrap());
    files
        .host
        .cleanup_publications(
            &access,
            &FileRetention {
                candidates: &Default::default(),
                protected: &Default::default(),
                all: true,
                now: crate::native_v2_runner::current_timestamp(),
            },
        )
        .unwrap();
    let handle: RunFileInput = serde_json::from_value(upload).unwrap();
    let RunFileInput::Staged { run_id, operation } = handle else {
        unreachable!()
    };
    assert!(
        files
            .host
            .read_pin(&access, &FilePin { run_id, operation }, &mut Vec::new())
            .is_err()
    );
    files
        .materialize_file_inputs(&RunId::new("run"), false)
        .unwrap();
    assert_eq!(read(&accepted, "left"), "accepted bytes");
    files.release_file_inputs(&RunId::new("run")).unwrap();
    assert!(files.host.pin_exists(&access, &unrelated).unwrap());
}

#[test]
fn caller_chosen_upload_identity_cannot_alias_an_accepted_private_pin() {
    use sha2::{Digest, Sha256};
    let root = tempdir();
    let files = files(root.path());
    let source = root.path().join("data.csv");
    std::fs::write(&source, "unrelated upload").unwrap();
    let colliding = FilePin {
        run_id: RunId::new("run"),
        operation: operation(&format!(
            "input-{:x}",
            Sha256::digest(serde_json::to_vec("/left").unwrap())
        )),
    };
    let access = FileAccess {
        target: &files.target,
        identity: &files.identity,
    };
    let mut transfer = Vec::new();
    pack(&source, &mut transfer).unwrap();
    files
        .host
        .stage_upload(&access, &colliding, &mut Cursor::new(transfer))
        .unwrap();
    std::fs::write(&source, "accepted original").unwrap();
    let value = json!({"left":source,"right":source});
    let frozen = files
        .freeze_file_inputs(InputAdmission {
            run_id: &RunId::new("run"),
            owner: &RunId::new("run"),
            key: &operation("inputs"),
            schema: &schema(),
            value: &value,
            local_root: Some(root.path()),
        })
        .unwrap();
    let accepted = frozen.value;
    files.host.release_upload(&access, &colliding).unwrap();
    files
        .materialize_file_inputs(&RunId::new("run"), true)
        .unwrap();
    assert_eq!(read(&accepted, "left"), "accepted original");
    assert_eq!(read(&accepted, "right"), "accepted original");
}
