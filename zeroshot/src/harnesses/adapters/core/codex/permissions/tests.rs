use super::*;

fn responses(config: Value) -> Vec<Value> {
    vec![
        json!({"result":{}}),
        json!({"result":{"config":config,"origins":{},"layers":[]}}),
        json!({"result":{"requirements":null}}),
    ]
}

fn empty() -> Value {
    json!({"sandbox_mode":null,"approval_policy":null,"projects":null,
        "approvals_reviewer":null,"sandbox_workspace_write":null,"permissions":null,
        "default_permissions":null,"profile":null,"profiles":{},"include_permissions_instructions":true})
}

#[test]
fn configured_provider_environment_is_discovered_without_values() {
    let mut config = empty();
    config["model_provider"] = json!("internal");
    config["model_providers"] = json!({
        "internal": {
            "env_key": "INTERNAL_API_KEY",
            "env_http_headers": {
                "X-Internal-Account": "INTERNAL_ACCOUNT",
                "X-Invalid": "NOT-A-NAME"
            }
        },
        "inactive": {"env_key": "INACTIVE_API_KEY"}
    });
    assert_eq!(
        provider_environment_names(&responses(config)),
        ["INTERNAL_ACCOUNT", "INTERNAL_API_KEY"]
    );
}

#[test]
fn only_missing_configured_provider_values_are_inherited_and_redacted() {
    let mut environment = BTreeMap::from([
        ("INTERNAL_API_KEY".to_owned(), "declared-secret".to_owned()),
        ("PATH".to_owned(), "/usr/bin".to_owned()),
    ]);
    let requested = std::cell::RefCell::new(Vec::new());
    let redactions = inherit_provider_environment(
        &mut environment,
        &[
            "INTERNAL_ACCOUNT".to_owned(),
            "INTERNAL_API_KEY".to_owned(),
            "MISSING".to_owned(),
        ],
        |name| {
            requested.borrow_mut().push(name.to_owned());
            match name {
                "INTERNAL_ACCOUNT" => Some("ambient-secret".to_owned()),
                "MISSING" => None,
                _ => panic!("declared values must take precedence"),
            }
        },
    );
    assert_eq!(requested.into_inner(), ["INTERNAL_ACCOUNT", "MISSING"]);
    assert_eq!(
        environment.get("INTERNAL_API_KEY").map(String::as_str),
        Some("declared-secret")
    );
    assert_eq!(
        environment.get("INTERNAL_ACCOUNT").map(String::as_str),
        Some("ambient-secret")
    );
    assert!(!environment.contains_key("MISSING"));
    assert_eq!(redactions, ["ambient-secret"]);
}

#[test]
fn only_unset_policy_receives_a_default() {
    let mut config = empty();
    config["web_search"] = json!("disabled");
    config["model_provider"] = json!("litellm");
    assert_eq!(
        permission_policy(&responses(config), Path::new("/project")),
        PermissionPolicy::Unset
    );
    for (key, value) in [
        ("sandbox_mode", json!("read-only")),
        ("sandbox_mode", json!("danger-full-access")),
        ("approval_policy", json!("never")),
        ("approvals_reviewer", json!("user")),
        ("sandbox_workspace_write", json!({"network_access":false})),
        ("permissions", json!({})),
        ("default_permissions", json!("locked")),
        ("profile", json!("custom")),
        ("browser_use", json!({"allow_history_access":false})),
        ("computer_use", json!({"default_app_access":"deny"})),
    ] {
        let mut config = empty();
        config[key] = value;
        assert_ne!(
            permission_policy(&responses(config), Path::new("/project")),
            PermissionPolicy::Unset,
            "{key}"
        );
    }
}

#[test]
fn native_empty_configuration_and_authored_network_policy() {
    let native: Vec<Value> = serde_json::from_str(include_str!("../permissions-empty-0.155.json"))
        .expect("recorded native config responses");
    assert_eq!(
        permission_policy(&native, Path::new("/native-fixture/project")),
        PermissionPolicy::Unset
    );
    for features in [
        json!({"network_proxy":true}),
        json!({"network_proxy":false}),
        json!({"network_proxy":{"enabled":true,"domains":{"example.com":"allow"}}}),
        json!({"guardian_approval":true}),
        json!({"guardian_ext":true}),
        json!({"guardian_ext":false}),
        json!({"write_stdin_approval":true}),
        json!({"write_stdin_approval":false}),
        json!({"web_search_cached":true}),
    ] {
        let mut response = native.clone();
        response[1]["result"]["config"]["features"] = features;
        assert_ne!(
            permission_policy(&response, Path::new("/native-fixture/project")),
            PermissionPolicy::Unset
        );
    }
    let mut response = native;
    response[1]["result"]["config"]["auto_review"] = json!({"policy":"configured"});
    assert_ne!(
        permission_policy(&response, Path::new("/native-fixture/project")),
        PermissionPolicy::Unset
    );
}

#[test]
fn cached_search_is_not_promoted_to_live_by_the_default() {
    for mode in ["cached", "disabled", "live"] {
        let mut response = responses(empty());
        response[1]["result"]["config"]["web_search"] = json!(mode);
        assert_eq!(
            permission_policy(&response, Path::new("/project")),
            if mode == "cached" {
                PermissionPolicy::Configured
            } else {
                PermissionPolicy::Unset
            }
        );
    }
    let mut response = responses(empty());
    response[1]["result"]["layers"] = json!([{"config":{"web_search":"cached"}}]);
    assert_ne!(
        permission_policy(&response, Path::new("/project")),
        PermissionPolicy::Unset
    );
}

#[test]
fn trust_and_requirements_are_not_overridden() {
    let mut config = empty();
    config["projects"] = json!({"/project":{"trust_level":"untrusted"}});
    assert_ne!(
        permission_policy(&responses(config.clone()), Path::new("/project/nested")),
        PermissionPolicy::Unset
    );
    assert_eq!(
        permission_policy(&responses(config), Path::new("/another")),
        PermissionPolicy::Unset
    );
    let mut response = responses(empty());
    response[2]["result"]["requirements"] =
        json!({"enforceResidency":"us","allowedWebSearchModes":["cached"]});
    assert_eq!(
        permission_policy(&response, Path::new("/project")),
        PermissionPolicy::Unset
    );
    for requirements in [
        json!({"allowedSandboxModes":["readOnly"]}),
        json!({"allowed_sandbox_modes":["read-only"]}),
        json!({"future_requirement":{}}),
        json!(false),
    ] {
        let mut response = responses(empty());
        response[2]["result"]["requirements"] = requirements;
        assert_ne!(
            permission_policy(&response, Path::new("/project")),
            PermissionPolicy::Unset
        );
    }
}

#[test]
fn configured_policy_is_distinct_from_unavailable_inspection() {
    let mut config = empty();
    config["sandbox_mode"] = json!("read-only");
    assert_eq!(
        permission_policy(&responses(config), Path::new("/project")),
        PermissionPolicy::Configured
    );
    for response in [responses(json!({})), vec![], responses(Value::Null)] {
        assert_eq!(
            permission_policy(&response, Path::new("/project")),
            PermissionPolicy::Unavailable
        );
    }
    let mut response = responses(empty());
    response[1]["result"]["layers"] = json!([{}]);
    assert_eq!(
        permission_policy(&response, Path::new("/project")),
        PermissionPolicy::Unavailable
    );
    response[1]["result"]["layers"] = json!([]);
    response[2]["result"]["requirements"] = json!(false);
    assert_eq!(
        permission_policy(&response, Path::new("/project")),
        PermissionPolicy::Unavailable
    );
}

#[test]
fn malformed_and_layer_configuration_prevent_escalation() {
    for response in [
        vec![],
        responses(json!({})),
        vec![json!({}), json!({}), json!({})],
    ] {
        assert_ne!(
            permission_policy(&response, Path::new("/project")),
            PermissionPolicy::Unset
        );
    }
    let mut response = responses(empty());
    response[1]["result"]["layers"] = json!([{"config":{"approval_policy":"on-request"}}]);
    assert_ne!(
        permission_policy(&response, Path::new("/project")),
        PermissionPolicy::Unset
    );
    response[1]["result"]["layers"] = json!([]);
    response[2] = json!({"error":{"code":-32601}});
    assert_ne!(
        permission_policy(&response, Path::new("/project")),
        PermissionPolicy::Unset
    );
}
