use std::ffi::OsString;

#[cfg(not(feature = "ui"))]
use openengine_cluster_testkit::assertions::AssertError;
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

struct RejectWrites;

impl std::io::Write for RejectWrites {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("output is closed"))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("output is closed"))
    }
}

fn assert_target_serve_is_part_of_the_public_command_schema() {
    let arguments = [
        "target",
        "serve",
        "--listen",
        "127.0.0.1:8080",
        "--public-origin",
        "http://127.0.0.1:8080",
        "--storage",
        "/tmp/zeroshot-target",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    let command = parse_native_v2_args(arguments).assert_value();
    assert!(matches!(command, NativeV2CliCommand::TargetServe(_)));
}

type RoutePair<'a> = (&'a [&'a str], &'a [&'a str]);

fn assert_route_pairs(route_pairs: &[RoutePair<'_>]) {
    for (local, remote) in route_pairs {
        for (arguments, expected_local) in [(local, true), (remote, false)] {
            let command =
                parse_native_v2_args(arguments.iter().copied().map(OsString::from)).assert_value();
            assert_eq!(
                is_local_command(&command),
                expected_local,
                "unexpected route for {arguments:?}"
            );
        }
    }
}

fn assert_run_routing_follows_the_authored_target() {
    assert_route_pairs(&[
        (
            &[
                "run",
                "--title",
                "Route locally",
                "--input",
                "input.json",
                "--template",
                "single-worker",
                "--runtime-config",
                "runtime.json",
            ],
            &[
                "run",
                "--title",
                "Route remotely",
                "--input",
                "input.json",
                "--template",
                "single-worker",
                "--runtime-config",
                "runtime.json",
                "--target",
                "prod",
            ],
        ),
        (&["list"], &["list", "--target", "prod"]),
        (
            &["status", "run-1"],
            &["status", "run-1", "--target", "prod"],
        ),
        (
            &["force-stop", "run-1"],
            &["force-stop", "run-1", "--target", "prod"],
        ),
        (
            &["resume", "run-1"],
            &["resume", "run-1", "--target", "prod"],
        ),
        (
            &["checkpoints", "run-1"],
            &["checkpoints", "run-1", "--target", "prod"],
        ),
        (
            &["discard-workspace", "run-1"],
            &["discard-workspace", "run-1", "--target", "prod"],
        ),
        (&["watch", "run-1"], &["watch", "run-1", "--target", "prod"]),
        (&["logs", "run-1"], &["logs", "run-1", "--target", "prod"]),
        (
            &["attach", "run-1", "exec-1"],
            &["attach", "run-1", "exec-1", "--target", "prod"],
        ),
    ]);
}

fn assert_management_routing_follows_the_authored_target() {
    assert_route_pairs(&[
        (
            &["connection", "list"],
            &["connection", "list", "--target", "prod"],
        ),
        (
            &["connection", "delete", "provider"],
            &["connection", "delete", "provider", "--target", "prod"],
        ),
        (
            &["connection", "set", "openai", "--field", "OPENAI_API_KEY"],
            &[
                "connection",
                "set",
                "openai",
                "--target",
                "prod",
                "--field",
                "OPENAI_API_KEY",
            ],
        ),
        (
            &["profile", "list"],
            &["profile", "list", "--target", "prod"],
        ),
        (
            &["profile", "show", "reviewer"],
            &["profile", "show", "reviewer", "--target", "prod"],
        ),
        (
            &["profile", "remove", "reviewer"],
            &["profile", "remove", "reviewer", "--target", "prod"],
        ),
        (
            &["profile", "default", "reviewer"],
            &["profile", "default", "reviewer", "--target", "prod"],
        ),
        (
            &[
                "profile",
                "set",
                "reviewer",
                "--template",
                "single-worker",
                "--runtime-config",
                "runtime.json",
            ],
            &[
                "profile",
                "set",
                "reviewer",
                "--target",
                "prod",
                "--template",
                "single-worker",
                "--runtime-config",
                "runtime.json",
            ],
        ),
    ]);

    let static_command =
        parse_native_v2_args(["template", "list"].map(OsString::from)).assert_value();
    assert!(!is_local_command(&static_command));
}

async fn assert_private_bootstrap_is_exact_and_public_startup_stays_public() {
    let public = Vec::<OsString>::new();
    assert!(
        private_controller_bootstrap(&public)
            .assert_value()
            .is_none()
    );
    assert!(!run_private_controller(&public).await.assert_value());

    for malformed in [
        vec![OsString::from(LOCAL_CONTROLLER_MODE)],
        vec![
            OsString::from(LOCAL_CONTROLLER_MODE),
            OsString::from("--bootstrap"),
        ],
        vec![
            OsString::from(LOCAL_CONTROLLER_MODE),
            OsString::from("--wrong"),
            OsString::from("bootstrap.json"),
        ],
        vec![
            OsString::from(LOCAL_CONTROLLER_MODE),
            OsString::from("--bootstrap"),
            OsString::from("bootstrap.json"),
            OsString::from("extra"),
        ],
    ] {
        assert!(matches!(
            private_controller_bootstrap(&malformed),
            Err(NativeV2CliError::Usage(message)) if message.contains("malformed")
        ));
    }
    assert!(matches!(
        run_private_controller(&[OsString::from(LOCAL_CONTROLLER_MODE)]).await,
        Err(ProcessError::Cli(NativeV2CliError::Usage(message))) if message.contains("malformed")
    ));
    let valid = [
        OsString::from(LOCAL_CONTROLLER_MODE),
        OsString::from("--bootstrap"),
        OsString::from("bootstrap.json"),
    ];
    assert_eq!(
        private_controller_bootstrap(&valid).assert_value(),
        Some(PathBuf::from("bootstrap.json"))
    );
    let missing_bootstrap = tempfile::tempdir()
        .assert_value()
        .path()
        .join("missing-bootstrap.json");
    let private = [
        OsString::from(LOCAL_CONTROLLER_MODE),
        OsString::from("--bootstrap"),
        missing_bootstrap.into_os_string(),
    ];
    assert!(matches!(
        run_private_controller(&private).await,
        Err(ProcessError::Portable(_))
    ));

    #[cfg(not(feature = "ui"))]
    assert!(
        dispatch(NativeV2CliCommand::Ui {
            listen: "127.0.0.1:0".parse().assert_value(),
            target: None,
        })
        .await
        .assert_error()
        .to_string()
        .contains("cargo build -p zeroshot --features ui")
    );
}

fn assert_process_diagnostics_preserve_cli_detail_and_classify_runtime_failures() {
    let cli = process_error_diagnostic(&ProcessError::Cli(NativeV2CliError::Usage(
        "invalid invocation".to_owned(),
    )));
    let cli = serde_json::to_value(cli).assert_value();
    assert_eq!(cli["kind"], "invalid_request");
    assert_eq!(cli["code"], "request.invalid");
    assert_eq!(cli["message"], "invalid invocation");

    let output = process_error_diagnostic(&ProcessError::Output(std::io::Error::other(
        "closed output",
    )));
    let output = serde_json::to_value(output).assert_value();
    assert_eq!(output["kind"], "target");
    assert_eq!(output["code"], "target.unavailable");
    assert_eq!(
        output["message"],
        "could not write CLI output: closed output"
    );
}

#[cfg(feature = "ui")]
async fn assert_ui_target_resolution_is_local_and_validates_stored_origins() {
    use native_v2_target::{TargetAccess, TargetRecord};

    let root = openengine_cluster_testkit::TemporaryDirectory::for_test("main-ui-target");
    let registry_path = root.path("targets.json");
    let registry = FileTargetRegistry::new(registry_path.clone());
    registry
        .insert(TargetRecord {
            id: "00000000-0000-4000-8000-000000000001".to_owned(),
            name: "local".to_owned(),
            origin: "http://127.0.0.1:4123".to_owned(),
            access: TargetAccess::Direct,
        })
        .assert_value();
    assert!(resolve_ui_target("local".to_owned(), || Ok(registry_path.clone())).is_ok());

    let error = build_ui_target(TargetRecord {
        id: "00000000-0000-4000-8000-000000000002".to_owned(),
        name: "invalid-origin".to_owned(),
        origin: "not an origin".to_owned(),
        access: TargetAccess::Direct,
    })
    .err()
    .assert_value();
    assert!(
        matches!(
            error,
            ProcessError::Target(TargetConnectorError::Authority(_))
        ),
        "unexpected stored-origin failure: {error}"
    );
    assert!(matches!(
        resolve_ui_target("missing".to_owned(), || Ok(registry_path)),
        Err(ProcessError::Target(TargetConnectorError::NotFound(name))) if name == "missing"
    ));
    assert!(matches!(
        resolve_ui_target("unreachable".to_owned(), || Err(
            TargetConnectorError::RegistryPath("test path")
        )),
        Err(ProcessError::Target(TargetConnectorError::RegistryPath(
            "test path"
        )))
    ));

    let service_error = serve_ui_with_registry_path(
        "127.0.0.1:0".parse().assert_value(),
        Some("missing".to_owned()),
        || Ok(root.path("targets.json")),
    )
    .await
    .err()
    .assert_value();
    assert!(matches!(
        service_error,
        ProcessError::Target(TargetConnectorError::NotFound(name)) if name == "missing"
    ));
}

fn validation_command(input: &std::path::Path, runtime: &std::path::Path) -> NativeV2CliCommand {
    parse_native_v2_args(vec![
        OsString::from("run"),
        OsString::from("--title"),
        OsString::from("Validate public routing"),
        OsString::from("--input"),
        input.as_os_str().to_owned(),
        OsString::from("--template"),
        OsString::from("single-worker"),
        OsString::from("--runtime-config"),
        runtime.as_os_str().to_owned(),
        OsString::from("--validate-only"),
    ])
    .assert_value()
}

async fn assert_static_dispatch_and_remaining_management_routes_are_exact() {
    dispatch(NativeV2CliCommand::Version).await.assert_value();
    let templates = parse_native_v2_args(["template", "list"].map(OsString::from)).assert_value();
    dispatch(templates).await.assert_value();

    let input = tempfile::Builder::new()
        .prefix("zeroshot-main-input-")
        .tempfile()
        .assert_value();
    let runtime = tempfile::Builder::new()
        .prefix("zeroshot-main-runtime-")
        .tempfile()
        .assert_value();
    std::fs::write(input.path(), r#"{"task":"validate public routing"}"#).assert_value();
    std::fs::write(
        runtime.path(),
        r#"{
            "harness":"codex",
            "provider":"openai",
            "size":"small",
            "nodes":{"worker":{"kind":"agent","model":"provider-model"}}
        }"#,
    )
    .assert_value();
    let validate = validation_command(input.path(), runtime.path());
    run_public_command(validate).await.assert_value();

    let static_error = execute_public_command(NativeV2CliCommand::Version, &mut RejectWrites)
        .await
        .err()
        .assert_value();
    assert!(matches!(
        static_error,
        ProcessError::Cli(NativeV2CliError::Output(_))
    ));

    let missing_input = input.path().with_extension("missing");
    let preflight_error = execute_public_command(
        validation_command(&missing_input, runtime.path()),
        &mut Vec::new(),
    )
    .await
    .err()
    .assert_value();
    assert!(
        matches!(
            preflight_error,
            ProcessError::Cli(NativeV2CliError::Read { kind: "input", ref path, .. })
                if *path == missing_input
        ),
        "unexpected validation failure: {preflight_error}"
    );

    let root = openengine_cluster_testkit::TemporaryDirectory::for_test("main-local-backend");
    let backend = LocalCliBackend::new(
        root.path("state"),
        std::env::current_exe().assert_value(),
        std::env::current_dir().assert_value(),
        PathBuf::from("git"),
    );
    let invalid_status =
        parse_native_v2_args(["status", "run-1"].map(OsString::from)).assert_value();
    let backend_error = execute_backend_command(
        invalid_status,
        &backend,
        &mut CtrlCDetachSignal,
        &mut Vec::new(),
    )
    .await
    .err()
    .assert_value();
    assert!(matches!(
        backend_error,
        ProcessError::Cli(NativeV2CliError::Local(message))
            if message.contains("not a local controller identity")
    ));

    let remote_list =
        parse_native_v2_args(["list", "--target", "missing"].map(OsString::from)).assert_value();
    let connector = NativeV2TargetConnector::new(
        FileTargetRegistry::new(root.path("targets.json")),
        TargetHttpControlAuthority::production().assert_value(),
        TargetOecpWebSocketDialer,
    );
    let backend = NamedTargetCliBackend::new(connector);
    let named_error = execute_backend_command(
        remote_list,
        &backend,
        &mut CtrlCDetachSignal,
        &mut Vec::new(),
    )
    .await
    .err()
    .assert_value();
    assert!(matches!(
        named_error,
        ProcessError::Cli(NativeV2CliError::Target(message))
            if message.contains("missing") && message.contains("not found")
    ));
}

#[tokio::test]
async fn wave10_cli_contract_process_dispatch_and_routing_matrix_is_exact() {
    assert_target_serve_is_part_of_the_public_command_schema();
    assert_run_routing_follows_the_authored_target();
    assert_management_routing_follows_the_authored_target();
    assert_private_bootstrap_is_exact_and_public_startup_stays_public().await;
    assert_static_dispatch_and_remaining_management_routes_are_exact().await;
    assert_process_diagnostics_preserve_cli_detail_and_classify_runtime_failures();
    #[cfg(feature = "ui")]
    assert_ui_target_resolution_is_local_and_validates_stored_origins().await;
}

#[tokio::test]
async fn service_dispatch_preserves_public_listener_and_origin_refusals() {
    #[cfg(feature = "ui")]
    {
        let error = dispatch(NativeV2CliCommand::Ui {
            listen: "0.0.0.0:0".parse().assert_value(),
            target: None,
        })
        .await
        .err()
        .assert_value();
        assert!(matches!(error, ProcessError::Cli(_)));
        assert!(error.to_string().contains("loopback"));
    }

    let root = openengine_cluster_testkit::TemporaryDirectory::for_test(
        "main-target-serve-invalid-origin",
    );
    let error = dispatch(NativeV2CliCommand::TargetServe(
        zeroshot_engine::native_v2_cli::TargetServe {
            listen: "127.0.0.1:0".parse().assert_value(),
            public_origin: "https://target.example/private".to_owned(),
            storage: root.path("storage"),
            bootstrap_key_file: None,
        },
    ))
    .await
    .err()
    .assert_value();
    assert!(matches!(
        error,
        ProcessError::Serve(TargetServeError::InvalidOrigin(_))
    ));
    assert!(!root.path("storage").exists());
}
