use std::ffi::OsString;

#[cfg(not(feature = "ui"))]
use openengine_cluster_testkit::assertions::AssertError;
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

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

    let malformed = [
        OsString::from(LOCAL_CONTROLLER_MODE),
        OsString::from("--wrong"),
        OsString::from("bootstrap.json"),
    ];
    assert!(matches!(
        private_controller_bootstrap(&malformed),
        Err(NativeV2CliError::Usage(message)) if message.contains("malformed")
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
    let validate = parse_native_v2_args(vec![
        OsString::from("run"),
        OsString::from("--title"),
        OsString::from("Validate public routing"),
        OsString::from("--input"),
        input.path().as_os_str().to_owned(),
        OsString::from("--template"),
        OsString::from("single-worker"),
        OsString::from("--runtime-config"),
        runtime.path().as_os_str().to_owned(),
        OsString::from("--validate-only"),
    ])
    .assert_value();
    run_public_command(validate).await.assert_value();
}

#[tokio::test]
async fn wave10_cli_contract_process_dispatch_and_routing_matrix_is_exact() {
    assert_target_serve_is_part_of_the_public_command_schema();
    assert_run_routing_follows_the_authored_target();
    assert_management_routing_follows_the_authored_target();
    assert_private_bootstrap_is_exact_and_public_startup_stays_public().await;
    assert_static_dispatch_and_remaining_management_routes_are_exact().await;
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
