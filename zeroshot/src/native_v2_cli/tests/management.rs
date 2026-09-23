use openengine_cluster_protocol::RunProfileScope;

use super::*;

async fn execute_json(command: NativeV2CliCommand, backend: &FakeBackend) -> Value {
    let mut output = Vec::new();
    assert_eq!(
        execute_native_v2_cli(command, backend, &mut NeverDetach, &mut output)
            .await
            .assert_value(),
        CliOutcome::Completed
    );
    serde_json::from_slice(&output).assert_value()
}

fn assert_profile_calls(calls: &[Call]) {
    assert!(matches!(
        calls,
        [
            Call::ProfileList { target, request: list },
            Call::ProfileShow {
                target: show_target,
                selector: show,
            },
            Call::ProfileSet {
                target: set_target,
                request: set,
            },
            Call::ProfileDelete {
                target: delete_target,
                selector: delete,
            },
            Call::ProfileDefault {
                target: named_target,
                request: named,
            },
            Call::ProfileDefault {
                target: clear_target,
                request: clear,
            },
        ] if target.as_deref() == Some("prod")
            && show_target == target
            && set_target == target
            && delete_target == target
            && named_target == target
            && clear_target == target
            && list.scope == RunProfileScope::Org
            && show.scope == RunProfileScope::Org
            && show.name.as_str() == "beta"
            && set.scope == RunProfileScope::Org
            && set.name.as_str() == "beta"
            && set.set_default
            && delete.scope == RunProfileScope::Org
            && delete.name.as_str() == "beta"
            && named.name.as_ref().map(|name| name.as_str()) == Some("alpha")
            && clear.name.is_none()
    ));
}

#[tokio::test]
async fn management_contract_routes_profile_commands_and_json() {
    let backend = FakeBackend::default();
    let route = ["--target", "prod", "--scope", "org"];

    let listed = execute_json(
        parse_native_v2_args(args(&[
            "profile", "list", route[0], route[1], route[2], route[3],
        ]))
        .assert_value(),
        &backend,
    )
    .await;
    assert_eq!(listed.pointer("/profiles/0/name"), Some(&json!("alpha")));
    assert_eq!(listed.pointer("/profiles/0/isDefault"), Some(&json!(true)));

    let shown = execute_json(
        parse_native_v2_args(args(&[
            "profile", "show", "beta", route[0], route[1], route[2], route[3],
        ]))
        .assert_value(),
        &backend,
    )
    .await;
    assert_eq!(shown.pointer("/name"), Some(&json!("beta")));
    assert_eq!(shown.pointer("/scope"), Some(&json!("org")));

    let files = FixtureFiles::new(graph(), json!({"task":"profile input is not persisted"}));
    let set = parse_native_v2_args(vec![
        OsString::from("profile"),
        OsString::from("set"),
        OsString::from("beta"),
        OsString::from("--graph"),
        files.graph.as_os_str().to_owned(),
        OsString::from("--runtime-config"),
        files.runtime.as_os_str().to_owned(),
        OsString::from("--default"),
        OsString::from("--target"),
        OsString::from("prod"),
        OsString::from("--scope"),
        OsString::from("org"),
    ])
    .assert_value();
    let stored = execute_json(set, &backend).await;
    assert_eq!(stored.pointer("/profile/name"), Some(&json!("beta")));
    assert_eq!(stored.pointer("/profile/isDefault"), Some(&json!(true)));
    assert_eq!(
        stored.pointer("/profile/graph/profile"),
        Some(&json!("openengine.graph.full/v1"))
    );

    let removed = execute_json(
        parse_native_v2_args(args(&[
            "profile", "remove", "beta", route[0], route[1], route[2], route[3],
        ]))
        .assert_value(),
        &backend,
    )
    .await;
    assert_eq!(removed, json!({"deleted":true}));

    for (name, expected) in [(Some("alpha"), json!("alpha")), (None, Value::Null)] {
        let mut command = vec!["profile", "default"];
        command.extend(name);
        command.extend(route);
        let selected = execute_json(
            parse_native_v2_args(args(&command)).assert_value(),
            &backend,
        )
        .await;
        assert_eq!(selected.pointer("/name"), Some(&expected));
        assert_eq!(selected.pointer("/scope"), Some(&json!("org")));
    }

    assert_profile_calls(&backend.calls());
}
