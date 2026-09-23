use super::*;

fn responses(settings: Value) -> Vec<Value> {
    vec![
        json!({"type":"control_response","response":{"subtype":"success",
            "response":{"current_permission_mode":"default"}}}),
        json!({"type":"control_response","response":{"subtype":"success",
            "response":settings}}),
    ]
}

#[test]
fn native_empty_configuration_receives_the_default() {
    let responses: Vec<Value> =
        serde_json::from_str(include_str!("../permissions-empty-2.1.237.json"))
            .expect("recorded native configuration responses");
    assert_eq!(permission_policy(&responses), PermissionPolicy::Unset);
}

#[test]
fn only_unconfigured_policy_receives_the_permissive_default() {
    for effective in [
        json!({}),
        json!({"permissions":{},"sandbox":{}}),
        json!({"model":"opaque-model","disableAllHooks":true,"env":{"ANTHROPIC_BASE_URL":"https://proxy.invalid"}}),
    ] {
        assert_eq!(
            permission_policy(&responses(
                json!({"effective":effective,"sources":[],"errors":[]})
            )),
            PermissionPolicy::Unset
        );
    }
    for effective in [
        json!({"permissions":{"defaultMode":"default"}}),
        json!({"permissions":{"deny":["Bash"]}}),
        json!({"permissions":{"allow":[]}}),
        json!({"sandbox":{"enabled":false}}),
        json!({"permissions":{"disableBypassPermissionsMode":"disable"}}),
        json!({"allowManagedPermissionRulesOnly":true}),
        json!({"permissions":null}),
        json!({"env":{"CLAUDE_CODE_FORCE_SANDBOX":"yes"}}),
    ] {
        assert_ne!(
            permission_policy(&responses(json!({"effective":effective,"sources":[]}))),
            PermissionPolicy::Unset
        );
    }
}

#[test]
fn omitted_policy_in_effective_settings_does_not_hide_a_configured_source() {
    for source in [
        "userSettings",
        "projectSettings",
        "localSettings",
        "policySettings",
        "futureSettings",
    ] {
        assert_ne!(
            permission_policy(&responses(json!({"effective":{},"sources":[
                {"source":source,"settings":{"permissions":{"defaultMode":"plan"}}}
            ]}))),
            PermissionPolicy::Unset
        );
    }
    assert_eq!(
        permission_policy(&responses(
            json!({"effective":{"disableAllHooks":true},"sources":[
                {"source":"flagSettings","settings":{"disableAllHooks":true}}
            ]})
        )),
        PermissionPolicy::Unset
    );
}

#[test]
fn malformed_or_incomplete_configuration_never_enables_bypass() {
    for settings in [
        json!({}),
        json!({"effective":{}}),
        json!({"effective":null,"sources":[]}),
        json!({"effective":{},"sources":[{}]}),
        json!({"effective":{},"sources":[{"source":"userSettings","settings":null}]}),
        json!({"effective":{},"sources":[],"errors":[{"message":"Invalid or malformed JSON"}]}),
        json!({"effective":{},"sources":[],"errors":null}),
    ] {
        assert_ne!(
            permission_policy(&responses(settings)),
            PermissionPolicy::Unset
        );
    }
    let mut values = responses(json!({"effective":{},"sources":[]}));
    values[0]["response"]["response"]["current_permission_mode"] = json!("plan");
    assert_ne!(permission_policy(&values), PermissionPolicy::Unset);
    values[0]["response"]["response"]["current_permission_mode"] = json!("default");
    values[1]["response"]["subtype"] = json!("error");
    assert_ne!(permission_policy(&values), PermissionPolicy::Unset);
    assert_ne!(permission_policy(&[]), PermissionPolicy::Unset);
}

#[test]
fn configured_policy_is_distinct_from_unavailable_inspection() {
    assert_eq!(
        permission_policy(&responses(
            json!({"effective":{"permissions":{"defaultMode":"plan"}},"sources":[]})
        )),
        PermissionPolicy::Configured
    );
    for settings in [
        json!({}),
        json!({"effective":{},"sources":[{}]}),
        json!({"effective":{},"sources":[],"errors":["invalid"]}),
    ] {
        assert_eq!(
            permission_policy(&responses(settings)),
            PermissionPolicy::Unavailable
        );
    }
    assert_eq!(permission_policy(&[]), PermissionPolicy::Unavailable);
    let mut values = responses(json!({"effective":{},"sources":[]}));
    values[0]["response"]["response"]["current_permission_mode"] = Value::Null;
    assert_eq!(permission_policy(&values), PermissionPolicy::Unavailable);
    values[0]["response"]["response"]["current_permission_mode"] = json!("plan");
    assert_eq!(permission_policy(&values), PermissionPolicy::Configured);
}

#[test]
fn explicit_command_and_environment_policy_remains_authoritative() {
    assert!(configured_arguments(&["--permission-mode=plan".to_owned()]));
    assert!(configured_arguments(&[
        "--settings".to_owned(),
        "settings.json".to_owned()
    ]));
    assert!(!configured_arguments(&[
        "--model".to_owned(),
        "opaque-model".to_owned()
    ]));
    for value in ["1", "true", " TRUE ", "yes", "on"] {
        assert!(permission_environment("CLAUDE_CODE_FORCE_SANDBOX", value));
    }
    for value in ["", "0", "false", "off", "no"] {
        assert!(!permission_environment("CLAUDE_CODE_FORCE_SANDBOX", value));
    }
    assert!(permission_environment(
        "CLAUDE_BG_SESSION_PERMISSION_RULES",
        r#"{"deny":["Bash"]}"#
    ));
}
