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

fn assert_process_routing_follows_the_authored_target() {
    let cases: &[(&[&str], bool)] = &[
        (&["list"], true),
        (&["list", "--target", "prod"], false),
        (&["status", "run-1"], true),
        (&["force-stop", "run-1", "--target", "prod"], false),
        (&["resume", "run-1"], true),
        (&["discard-workspace", "run-1", "--target", "prod"], false),
        (&["watch", "run-1"], true),
        (&["logs", "run-1", "--target", "prod"], false),
        (&["attach", "run-1", "exec-1"], true),
        (&["connection", "list"], true),
        (
            &[
                "connection",
                "set",
                "openai",
                "--target",
                "prod",
                "--field",
                "OPENAI_API_KEY",
            ],
            false,
        ),
        (&["profile", "show", "reviewer"], true),
        (
            &["profile", "default", "reviewer", "--target", "prod"],
            false,
        ),
        (&["template", "list"], false),
    ];
    for (arguments, expected_local) in cases {
        let command =
            parse_native_v2_args(arguments.iter().copied().map(OsString::from)).assert_value();
        assert_eq!(
            is_local_command(&command),
            *expected_local,
            "unexpected route for {arguments:?}"
        );
    }
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
        serve_ui("127.0.0.1:0".parse().assert_value(), None)
            .await
            .assert_error()
            .to_string()
            .contains("cargo build -p zeroshot --features ui")
    );
}

async fn assert_static_dispatch_and_remaining_management_routes_are_exact() {
    for (arguments, expected_local) in [
        (vec!["connection", "list"], true),
        (vec!["connection", "list", "--target", "prod"], false),
        (vec!["connection", "delete", "provider"], true),
        (
            vec!["connection", "delete", "provider", "--target", "prod"],
            false,
        ),
        (
            vec![
                "profile",
                "set",
                "reviewer",
                "--template",
                "single-worker",
                "--runtime-config",
                "runtime.json",
            ],
            true,
        ),
        (
            vec![
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
            false,
        ),
        (vec!["profile", "list"], true),
        (vec!["profile", "list", "--target", "prod"], false),
        (vec!["profile", "show", "reviewer"], true),
        (
            vec!["profile", "show", "reviewer", "--target", "prod"],
            false,
        ),
        (vec!["profile", "remove", "reviewer"], true),
        (
            vec!["profile", "remove", "reviewer", "--target", "prod"],
            false,
        ),
        (vec!["profile", "default", "reviewer"], true),
        (
            vec!["profile", "default", "reviewer", "--target", "prod"],
            false,
        ),
    ] {
        let command =
            parse_native_v2_args(arguments.into_iter().map(OsString::from)).assert_value();
        assert_eq!(is_local_command(&command), expected_local);
    }

    dispatch(NativeV2CliCommand::Version).await.assert_value();
    let templates = parse_native_v2_args(["template", "list"].map(OsString::from)).assert_value();
    dispatch(templates).await.assert_value();
}

#[tokio::test]
async fn wave10_cli_contract_process_dispatch_and_routing_matrix_is_exact() {
    assert_target_serve_is_part_of_the_public_command_schema();
    assert_process_routing_follows_the_authored_target();
    assert_private_bootstrap_is_exact_and_public_startup_stays_public().await;
    assert_static_dispatch_and_remaining_management_routes_are_exact().await;
}
