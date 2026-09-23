use std::collections::BTreeMap;
use std::io::Write;

use openengine_cluster_protocol::{
    ConnectionKey, ConnectionScope, EnvironmentVariableName, StaticConnectionValues,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::*;
use crate::native_v2_cli::local::LocalCliBackend;
use crate::native_v2_cli::tests::support::{Call, FakeBackend};
use crate::native_v2_cli::ConnectionRoute;

struct UnavailableOutput;

impl Write for UnavailableOutput {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("output unavailable"))
    }
}

#[tokio::test]
async fn wave7_cli_contract_management_routes_values_without_secret_output() {
    let backend = FakeBackend::default();
    let route = ConnectionRoute {
        target: Some("prod".to_owned()),
        scope: ConnectionScope::Org,
    };

    let mut listed = Vec::new();
    assert_eq!(
        execute_connection(
            NativeV2CliCommand::ConnectionList(route.clone()),
            &backend,
            &mut listed,
        )
        .await
        .assert_value(),
        CliOutcome::Completed
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&listed).assert_value(),
        json!({"connections":[{
            "key":"openai",
            "scope":"org",
            "kind":"static",
            "fields":["OPENAI_API_KEY"]
        }]})
    );

    let secret = "provider-secret";
    let field = EnvironmentVariableName::new("OPENAI_API_KEY").assert_value();
    let values = StaticConnectionValues::new(BTreeMap::from([(field.clone(), secret.to_owned())]))
        .assert_value();
    let mut stored = Vec::new();
    store_connection_values(
        &route,
        ConnectionSetRequest {
            key: ConnectionKey::new("openai").assert_value(),
            scope: route.scope,
            values,
        },
        &backend,
        &mut stored,
    )
    .await
    .assert_value();
    let stored_json = serde_json::from_slice::<Value>(&stored).assert_value();
    assert_eq!(
        stored_json,
        json!({"connection":{
            "key":"openai",
            "scope":"org",
            "kind":"static",
            "fields":["OPENAI_API_KEY"]
        }})
    );
    assert!(!String::from_utf8(stored).assert_value().contains(secret));

    let mut deleted = Vec::new();
    execute_connection(
        NativeV2CliCommand::ConnectionDelete {
            route,
            key: ConnectionKey::new("openai").assert_value(),
        },
        &backend,
        &mut deleted,
    )
    .await
    .assert_value();
    assert_eq!(
        serde_json::from_slice::<Value>(&deleted).assert_value(),
        json!({"deleted":true})
    );

    let calls = backend.calls();
    assert!(matches!(
        calls.as_slice(),
        [
            Call::ConnectionList { target, request },
            Call::ConnectionSet {
                target: set_target,
                request: set_request,
            },
            Call::ConnectionDelete {
                target: delete_target,
                request: delete_request,
            },
        ] if target.as_deref() == Some("prod")
            && request.scope == ConnectionScope::Org
            && set_target == target
            && set_request.scope == ConnectionScope::Org
            && set_request.key.as_str() == "openai"
            && set_request.values.as_map().get(&field).map(String::as_str) == Some(secret)
            && delete_target == target
            && delete_request.scope == ConnectionScope::Org
            && delete_request.key.as_str() == "openai"
    ));
}

#[tokio::test]
async fn wave7_cli_contract_connection_dispatch_rejects_empty_and_wrong_operations() {
    let backend = FakeBackend::default();
    let rejected = execute_connection(
        NativeV2CliCommand::ConnectionSet(ConnectionSetCommand {
            route: ConnectionRoute {
                target: Some("prod".to_owned()),
                scope: ConnectionScope::Org,
            },
            key: ConnectionKey::new("empty").assert_value(),
            input: ConnectionInput::Prompt(Vec::new()),
        }),
        &backend,
        &mut Vec::new(),
    )
    .await;
    assert!(matches!(rejected, Err(NativeV2CliError::Usage(_))));

    assert!(matches!(
        execute_connection(
            NativeV2CliCommand::Version,
            &backend,
            &mut Vec::new()
        )
        .await,
        Err(NativeV2CliError::Usage(message))
            if message == "expected a connection operation"
    ));
}

#[test]
fn wave6_cli_contract_connection_inputs_are_bounded_and_fail_closed() {
    let fields = ["OPENAI_API_KEY", "OPENAI_ORG_ID"]
        .map(|name| EnvironmentVariableName::new(name).assert_value())
        .to_vec();
    let mut prompted = Vec::new();
    let values = read_connection_values_with(
        ConnectionInput::Prompt(fields.clone()),
        |field| {
            prompted.push(field.as_str().to_owned());
            Ok(format!("value-for-{}", field.as_str()))
        },
        || panic!("prompt input must not read stdin"),
    )
    .assert_value();
    assert_eq!(prompted, ["OPENAI_API_KEY", "OPENAI_ORG_ID"]);
    assert_eq!(values.as_map().len(), 2);

    let json = read_connection_values_with(
        ConnectionInput::JsonStdin,
        |_| panic!("JSON input must not prompt"),
        || Ok(r#"{"OPENAI_API_KEY":"secret"}"#.to_owned()),
    )
    .assert_value();
    assert_eq!(
        json.as_map().get(&fields[0]).map(String::as_str),
        Some("secret")
    );

    for encoded in [
        "not-json",
        r#"{"not an environment name":"secret"}"#,
        r#"{"OPENAI_API_KEY":""}"#,
    ] {
        assert!(matches!(
            read_connection_values_with(
                ConnectionInput::JsonStdin,
                |_| unreachable!(),
                || Ok(encoded.to_owned()),
            ),
            Err(NativeV2CliError::Usage(_))
        ));
    }

    let io_error = read_connection_values_with(
        ConnectionInput::JsonStdin,
        |_| unreachable!(),
        || Err(std::io::Error::other("stdin unavailable")),
    )
    .unwrap_err();
    assert!(matches!(io_error, NativeV2CliError::Output(_)));

    let mut prompts = 0;
    let prompt_error = read_connection_values_with(
        ConnectionInput::Prompt(fields),
        |_| {
            prompts += 1;
            if prompts == 2 {
                Err(std::io::Error::other("terminal unavailable"))
            } else {
                Ok("first-secret".to_owned())
            }
        },
        || panic!("prompt input must not read stdin"),
    )
    .unwrap_err();
    assert!(matches!(prompt_error, NativeV2CliError::Output(_)));
    assert_eq!(prompts, 2, "prompting must stop at the first I/O failure");
}

#[tokio::test]
async fn connection_mutations_surface_output_failure_after_backend_completion() {
    let backend = FakeBackend::default();
    let route = ConnectionRoute {
        target: Some("prod".to_owned()),
        scope: ConnectionScope::Org,
    };
    let field = EnvironmentVariableName::new("OPENAI_API_KEY").assert_value();
    let values =
        StaticConnectionValues::new(BTreeMap::from([(field, "provider-secret".to_owned())]))
            .assert_value();

    let listed = execute_connection(
        NativeV2CliCommand::ConnectionList(route.clone()),
        &backend,
        &mut UnavailableOutput,
    )
    .await;
    assert!(matches!(listed, Err(NativeV2CliError::Output(_))));

    let stored = store_connection_values(
        &route,
        ConnectionSetRequest {
            key: ConnectionKey::new("openai").assert_value(),
            scope: route.scope,
            values,
        },
        &backend,
        &mut UnavailableOutput,
    )
    .await;
    assert!(matches!(stored, Err(NativeV2CliError::Output(_))));

    let deleted = execute_connection(
        NativeV2CliCommand::ConnectionDelete {
            route: route.clone(),
            key: ConnectionKey::new("openai").assert_value(),
        },
        &backend,
        &mut UnavailableOutput,
    )
    .await;
    assert!(matches!(deleted, Err(NativeV2CliError::Output(_))));
    assert_eq!(backend.calls().len(), 3);

    let root = tempfile::tempdir().assert_value();
    let local = LocalCliBackend::new(
        root.path().to_path_buf(),
        "zeroshot".into(),
        root.path().to_path_buf(),
        "git".into(),
    );
    let mut refused_output = Vec::new();
    let listed = execute_connection(
        NativeV2CliCommand::ConnectionList(route.clone()),
        &local,
        &mut refused_output,
    )
    .await;
    assert!(matches!(listed, Err(NativeV2CliError::Local(_))));
    let stored = store_connection_values(
        &route,
        ConnectionSetRequest {
            key: ConnectionKey::new("openai").assert_value(),
            scope: route.scope,
            values: StaticConnectionValues::new(BTreeMap::from([(
                EnvironmentVariableName::new("OPENAI_API_KEY").assert_value(),
                "provider-secret".to_owned(),
            )]))
            .assert_value(),
        },
        &local,
        &mut refused_output,
    )
    .await;
    assert!(matches!(stored, Err(NativeV2CliError::Local(_))));
    let deleted = execute_connection(
        NativeV2CliCommand::ConnectionDelete {
            route,
            key: ConnectionKey::new("openai").assert_value(),
        },
        &local,
        &mut refused_output,
    )
    .await;
    assert!(matches!(deleted, Err(NativeV2CliError::Local(_))));
    assert!(
        refused_output.is_empty(),
        "backend refusal must not emit a partial response"
    );
}
