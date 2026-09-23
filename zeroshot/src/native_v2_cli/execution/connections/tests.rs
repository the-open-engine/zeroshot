use std::collections::BTreeMap;

use openengine_cluster_protocol::{
    ConnectionKey, ConnectionScope, EnvironmentVariableName, StaticConnectionValues,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::*;
use crate::native_v2_cli::tests::support::{Call, FakeBackend};
use crate::native_v2_cli::ConnectionRoute;

#[tokio::test]
async fn management_contract_routes_connection_values_without_secret_output() {
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
        route.clone(),
        ConnectionKey::new("openai").assert_value(),
        values,
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
}
