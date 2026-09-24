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
        json!({"agent":null}),
        json!({"permissions":null}),
        json!({"env":"CLAUDE_CODE_FORCE_SANDBOX=true"}),
        json!({"env":{"CLAUDE_CODE_FORCE_SANDBOX":true}}),
        json!({"env":{"CLAUDE_CODE_FORCE_SANDBOX":"yes"}}),
    ] {
        assert_eq!(
            permission_policy(&responses(json!({"effective":effective,"sources":[]}))),
            PermissionPolicy::Configured
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
        json!({"effective":{},"sources":{}}),
        json!({"effective":{},"sources":[{}]}),
        json!({"effective":{},"sources":[{"source":"","settings":{}}]}),
        json!({"effective":{},"sources":[{"source":7,"settings":{}}]}),
        json!({"effective":{},"sources":[{"source":"userSettings"}]}),
        json!({"effective":{},"sources":[{"source":"userSettings","settings":null}]}),
        json!({"effective":{},"sources":[],"errors":[{"message":"Invalid or malformed JSON"}]}),
        json!({"effective":{},"sources":[],"errors":null}),
    ] {
        assert_eq!(
            permission_policy(&responses(settings)),
            PermissionPolicy::Unavailable
        );
    }
    let mut values = responses(json!({"effective":{},"sources":[]}));
    values[1]["response"]["subtype"] = json!("error");
    assert_eq!(permission_policy(&values), PermissionPolicy::Unavailable);
    assert_eq!(permission_policy(&[]), PermissionPolicy::Unavailable);

    for malformed_response in [
        json!({}),
        json!({"type":7,"response":{"subtype":"success","response":{}}}),
        json!({"type":"future_control_response","response":{"subtype":"success","response":{}}}),
        json!({"type":"control_response","response":{}}),
        json!({"type":"control_response","response":{"subtype":7,"response":{}}}),
        json!({"type":"control_response","response":{"subtype":"error"}}),
        json!({"type":"control_response","response":{"subtype":"success"}}),
    ] {
        for response_index in [0, 1] {
            let mut values = responses(json!({"effective":{},"sources":[]}));
            values[response_index] = malformed_response.clone();
            assert_eq!(permission_policy(&values), PermissionPolicy::Unavailable);
        }
    }
}

#[test]
fn configured_policy_is_classified_exactly() {
    assert_eq!(
        permission_policy(&responses(
            json!({"effective":{"permissions":{"defaultMode":"plan"}},"sources":[]})
        )),
        PermissionPolicy::Configured
    );
    let mut values = responses(json!({"effective":{},"sources":[]}));
    values[0]["response"]["response"]["current_permission_mode"] = json!("plan");
    assert_eq!(permission_policy(&values), PermissionPolicy::Configured);
}

#[test]
fn explicit_command_and_environment_policy_remains_authoritative() {
    for argument in [
        "--permission-mode=plan",
        "--restricted",
        "--dangerously-skip-permissions",
        "--allow-dangerously-skip-permissions",
        "--allowedTools=Bash",
        "--allowed-tools=Bash",
        "--disallowedTools=WebFetch",
        "--disallowed-tools=WebFetch",
        "--permission-prompt-tool=approval",
        "--settings=settings.json",
        "--agent=reviewer",
        "--agents=reviewer",
    ] {
        assert!(
            configured_arguments(&[argument.to_owned()]),
            "permission control {argument} must suppress the permissive default"
        );
    }
    assert!(!configured_arguments(&[
        "--model".to_owned(),
        "opaque-model".to_owned()
    ]));

    for name in [
        "CLAUDE_CODE_FORCE_SANDBOX",
        "CLAUDE_CODE_RESTRICTED",
        "CLAUDE_CODE_AUTO_MODE_EXTERNAL_PERMISSIONS",
    ] {
        for value in ["1", "true", " TRUE ", "yes", "on"] {
            assert!(permission_environment(name, value));
        }
        for value in ["", "0", "false", "off", "no"] {
            assert!(!permission_environment(name, value));
        }
    }
    assert!(permission_environment(
        "CLAUDE_BG_SESSION_PERMISSION_RULES",
        r#"{"deny":["Bash"]}"#
    ));
    assert!(!permission_environment(
        "CLAUDE_BG_SESSION_PERMISSION_RULES",
        "  "
    ));
    assert!(!permission_environment("ANTHROPIC_API_KEY", "present"));

    let mut environment = BTreeMap::from([("ANTHROPIC_API_KEY".to_owned(), "present".to_owned())]);
    assert!(!configured_environment(&environment));
    environment.insert("CLAUDE_CODE_RESTRICTED".to_owned(), " true ".to_owned());
    assert!(configured_environment(&environment));
}
