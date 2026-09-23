use std::fs;

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[tokio::test]
async fn boundary_contract_readiness_rejects_malformed_or_mismatched_documents_and_times_out_immediately()
 {
    let root = tempfile::tempdir().assert_value();
    let storage = root.path().join("state");
    crate::execution::platform::create_private_directory(&storage).assert_value();
    let paths = PortableControllerPaths::new(&storage);
    let run_id = RunId::new("run-ready-contract");

    for ready in [
        PortableControllerReady {
            kind: "wrong-kind".to_owned(),
            run_id: run_id.clone(),
            socket: paths.socket(),
            pid: 1,
        },
        PortableControllerReady {
            kind: READY_KIND.to_owned(),
            run_id: run_id.clone(),
            socket: storage.join("wrong.sock"),
            pid: 1,
        },
    ] {
        let bytes = serde_json::to_vec(&ready).assert_value();
        write_private_new_file(&paths.ready(), &bytes).assert_value();
        assert!(matches!(
            read_ready(&paths),
            Err(PortableControllerError::Readiness)
        ));
        fs::remove_file(paths.ready()).assert_value();
    }

    write_private_new_file(&paths.ready(), br#"{"kind":"unknown","extra":true}"#).assert_value();
    assert!(matches!(
        read_ready(&paths),
        Err(PortableControllerError::Readiness)
    ));
    fs::remove_file(paths.ready()).assert_value();

    let expected = PortableControllerReady {
        kind: READY_KIND.to_owned(),
        run_id: run_id.clone(),
        socket: paths.socket(),
        pid: 7,
    };
    write_private_new_file(
        &paths.ready(),
        &serde_json::to_vec(&expected).assert_value(),
    )
    .assert_value();
    assert_eq!(
        wait_ready(&paths, &run_id, Duration::from_millis(50))
            .await
            .assert_value(),
        expected
    );
    fs::remove_file(paths.ready()).assert_value();

    assert!(matches!(
        wait_ready(&paths, &run_id, Duration::ZERO).await,
        Err(PortableControllerError::Readiness)
    ));
}

#[test]
fn boundary_contract_bootstrap_parse_failure_is_consumed_and_file_bounds_are_enforced() {
    let root = tempfile::tempdir().assert_value();
    let malformed = root.path().join("bootstrap.json");
    write_private_new_file(&malformed, b"{").assert_value();
    assert!(matches!(
        load_bootstrap_file(&malformed),
        Err(PortableControllerError::Bootstrap)
    ));
    assert!(
        !malformed.exists(),
        "even malformed bootstrap secrets are consumed"
    );

    let bounded = root.path().join("bounded");
    fs::write(&bounded, b"four").assert_value();
    assert_eq!(
        read_bounded_file(fs::File::open(&bounded).assert_value(), 4).assert_value(),
        b"four"
    );
    assert!(matches!(
        read_bounded_file(fs::File::open(&bounded).assert_value(), 3),
        Err(PortableControllerError::Bootstrap)
    ));

    #[cfg(unix)]
    assert!(matches!(
        read_bounded_file(fs::File::open(root.path()).assert_value(), 4),
        Err(PortableControllerError::Bootstrap)
    ));

    #[cfg(target_os = "linux")]
    assert!(matches!(
        read_bounded_file(fs::File::open("/proc/self/status").assert_value(), 1),
        Err(PortableControllerError::Bootstrap)
    ));
}

#[test]
fn boundary_contract_path_validation_distinguishes_regular_files_directories_symlinks_and_absence()
{
    let root = tempfile::tempdir().assert_value();
    assert!(require_absolute(root.path()).is_ok());
    assert!(matches!(
        require_absolute(Path::new("relative")),
        Err(PortableControllerError::Path)
    ));

    let missing = root.path().join("missing");
    assert!(validate_ledger_path(&missing).is_ok());
    assert!(remove_existing_regular_file(&missing).is_ok());

    let file = root.path().join("ledger");
    fs::write(&file, b"ledger").assert_value();
    assert!(validate_ledger_path(&file).is_ok());
    assert!(validate_existing_ledger_path(&file).is_ok());
    assert!(matches!(
        validate_existing_storage(&file),
        Err(PortableControllerError::LedgerPath)
    ));

    let directory = root.path().join("directory");
    fs::create_dir(&directory).assert_value();
    assert!(validate_existing_storage(&directory).is_ok());
    assert!(matches!(
        validate_ledger_path(&directory),
        Err(PortableControllerError::LedgerPath)
    ));
    assert!(matches!(
        validate_existing_ledger_path(&directory),
        Err(PortableControllerError::LedgerPath)
    ));
    assert!(matches!(
        remove_existing_regular_file(&directory),
        Err(PortableControllerError::EndpointPath)
    ));

    let removable = root.path().join("ready");
    fs::write(&removable, b"ready").assert_value();
    remove_existing_regular_file(&removable).assert_value();
    assert!(!removable.exists());

    #[cfg(unix)]
    {
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&file, &link).assert_value();
        assert!(matches!(
            validate_ledger_path(&link),
            Err(PortableControllerError::LedgerPath)
        ));
        assert!(matches!(
            validate_existing_ledger_path(&link),
            Err(PortableControllerError::LedgerPath)
        ));
        assert!(matches!(
            remove_existing_regular_file(&link),
            Err(PortableControllerError::EndpointPath)
        ));
        assert!(link.exists(), "unsafe endpoint entries are never removed");
    }
}

#[test]
fn coverage_contract_bootstrap_parent_and_private_read_refuse_ambiguous_filesystem_entries() {
    assert!(matches!(
        prepare_bootstrap_parent(Path::new("/")),
        Err(PortableControllerError::Path)
    ));

    let root = tempfile::tempdir().assert_value();
    let regular_parent = root.path().join("regular-parent");
    fs::write(&regular_parent, b"not a directory").assert_value();
    assert!(prepare_bootstrap_parent(&regular_parent.join("bootstrap.json")).is_err());

    let missing = root.path().join("missing-bootstrap.json");
    assert!(matches!(
        validate_private_bootstrap(&missing),
        Err(PortableControllerError::BootstrapPermissions)
    ));
    assert!(matches!(
        read_bounded_regular_file(root.path(), 16),
        Err(PortableControllerError::Bootstrap | PortableControllerError::Io(_))
    ));

    #[cfg(unix)]
    {
        let source = root.path().join("source");
        fs::write(&source, b"secret").assert_value();
        let link = root.path().join("bootstrap-link");
        std::os::unix::fs::symlink(&source, &link).assert_value();
        assert!(matches!(
            validate_private_bootstrap(&link),
            Err(PortableControllerError::BootstrapPermissions)
        ));
        assert!(matches!(
            read_bounded_regular_file(&link, 16),
            Err(PortableControllerError::Io(_))
        ));
    }
}
