use std::collections::BTreeMap;
#[cfg(unix)]
use std::sync::Arc;

use openengine_cluster_protocol::NodeName;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::*;
use crate::execution::SessionScope;
use crate::native_v2_capsule::provider_process::turn::ProviderTranscript;

#[cfg(unix)]
fn node_command() -> std::process::Command {
    let executable = crate::execution::platform::find_executable(
        "node",
        &BTreeMap::from([("PATH".to_owned(), std::env::var("PATH").assert_value())]),
        &std::env::current_dir().assert_value(),
    )
    .expect("Node.js must be installed on the test PATH");
    let mut command = std::process::Command::new(executable);
    command.env_clear();
    command
}

fn event(kind: &str, id: &str, message: &str) -> Value {
    json!({"type":kind,"sessionID":"ses_fixture","part":{"id":id,"messageID":message}})
}

fn text_event(id: &str, message: &str, answer: &str) -> Value {
    let mut event = event("text", id, message);
    event["part"]["text"] = json!(answer);
    event
}

fn finish_event(id: &str, message: &str, reason: &str) -> Value {
    let mut event = event("step_finish", id, message);
    event["part"]["reason"] = json!(reason);
    event["part"]["tokens"] =
        json!({"input":11,"output":7,"reasoning":0,"cache":{"read":3,"write":2}});
    event
}

fn push(transcript: &mut transcript::OpenCodeTranscript, event: &Value) {
    let mut bytes = serde_json::to_vec(event).assert_value();
    bytes.push(b'\n');
    transcript.push(&bytes);
}

#[test]
fn native_usage_accumulates_steps_and_deduplicates_part_updates() {
    let mut transcript = transcript::OpenCodeTranscript::new(None, vec![]);
    let first = finish_event("f1", "m1", "tool-calls");
    push(&mut transcript, &first);
    push(&mut transcript, &first);
    push(
        &mut transcript,
        &text_event("t2", "m2", "{\"response\":null}"),
    );
    push(&mut transcript, &finish_event("f2", "m2", "stop"));
    assert!(transcript.is_success());
    let usage = transcript.token_usage().assert_value();
    assert_eq!(usage.input_tokens.get(), 22);
    assert_eq!(usage.output_tokens.get(), 14);
    assert_eq!(usage.cache_read_input_tokens.assert_value().get(), 6);
    match transcript.finish(None).assert_value() {
        ProviderAttempt::Complete(result) => assert_eq!(result.message, "{\"response\":null}"),
        ProviderAttempt::Failed(_) => panic!("complete native step must settle successfully"),
    }
}

#[test]
fn malformed_usage_never_restores_an_earlier_partial_total() {
    let mut transcript = transcript::OpenCodeTranscript::new(None, vec![]);
    push(&mut transcript, &finish_event("f1", "m1", "tool-calls"));
    let mut malformed = finish_event("f2", "m2", "tool-calls");
    malformed["part"]["tokens"]["input"] = json!(-1);
    push(&mut transcript, &malformed);
    push(&mut transcript, &finish_event("f3", "m3", "stop"));
    assert!(transcript.token_usage().is_none());
}

#[test]
fn a_damaged_record_makes_later_usage_incomplete() {
    let mut transcript = transcript::OpenCodeTranscript::new(None, vec![]);
    push(&mut transcript, &finish_event("f1", "m1", "tool-calls"));
    transcript.push(b"{damaged-native-event}\n");
    push(&mut transcript, &text_event("t2", "m2", "done"));
    push(&mut transcript, &finish_event("f2", "m2", "stop"));
    assert!(transcript.is_success());
    assert!(transcript.token_usage().is_none());
}

#[test]
fn reasoning_tokens_are_counted_and_overflow_is_unknown() {
    for (reasoning, expected) in [
        (5, Some(12)),
        (openengine_cluster_protocol::MAX_SAFE_GENERATION, None),
    ] {
        let mut transcript = transcript::OpenCodeTranscript::new(None, vec![]);
        let mut finish = finish_event("f1", "m1", "stop");
        finish["part"]["tokens"]["reasoning"] = json!(reasoning);
        push(&mut transcript, &finish);
        assert_eq!(
            transcript
                .token_usage()
                .map(|usage| usage.output_tokens.get()),
            expected
        );
    }
}

#[test]
fn inline_custom_header_values_are_redacted_individually() {
    let environment = BTreeMap::from([("OPENCODE_CONFIG_CONTENT".to_owned(),
        json!({"provider":{"anthropic":{"options":{"headers":{"x-api-key":"header-secret","x-custom-auth":"custom-secret"}}}}}).to_string())]);
    let secrets = command::configuration_secrets(&environment);
    assert!(secrets.contains(&"header-secret".to_owned()));
    assert!(secrets.contains(&"custom-secret".to_owned()));
    let mut transcript = transcript::OpenCodeTranscript::new(None, secrets);
    push(
        &mut transcript,
        &json!({"type":"error","sessionID":"ses_fixture","error":{"data":{"message":"header-secret custom-secret"}}}),
    );
    match transcript.finish(None).assert_value() {
        ProviderAttempt::Failed(failure) => {
            assert!(!failure.diagnostic.contains("header-secret"));
            assert!(!failure.diagnostic.contains("custom-secret"));
        }
        ProviderAttempt::Complete(_) => panic!("native error cannot succeed"),
    }
}

#[test]
fn unknown_events_cannot_change_the_bound_session() {
    let mut transcript =
        transcript::OpenCodeTranscript::new(Some("ses_fixture".to_owned()), vec![]);
    push(
        &mut transcript,
        &json!({"type":"future_event","sessionID":false}),
    );
    push(&mut transcript, &text_event("t1", "m1", "done"));
    push(&mut transcript, &finish_event("f1", "m1", "stop"));
    assert!(transcript.is_success());
}

#[test]
fn a_changed_session_or_late_native_error_refuses_an_answer() {
    for changed in [true, false] {
        let mut transcript =
            transcript::OpenCodeTranscript::new(None, vec!["hidden-key".to_owned()]);
        push(&mut transcript, &text_event("t1", "m1", "done"));
        push(&mut transcript, &finish_event("f1", "m1", "stop"));
        let mut failure = json!({"type":"error","sessionID":"ses_fixture","error":{"data":{"message":"hidden-key rejected"}}});
        if changed {
            failure["sessionID"] = json!("ses_other");
        }
        push(&mut transcript, &failure);
        assert!(!transcript.is_success());
        match transcript.finish(None).assert_value() {
            ProviderAttempt::Failed(failure) => assert!(!failure.diagnostic.contains("hidden-key")),
            ProviderAttempt::Complete(_) => panic!("native failure must win over earlier answer"),
        }
    }
}

#[test]
fn final_record_without_a_newline_still_settles() {
    let mut transcript = transcript::OpenCodeTranscript::new(None, vec![]);
    push(&mut transcript, &text_event("t1", "m1", "done"));
    transcript.push(&serde_json::to_vec(&finish_event("f1", "m1", "stop")).assert_value());
    transcript.finish_stream();
    assert!(transcript.is_success());
}

#[test]
fn contributed_registration_is_available_only_in_the_local_catalog() {
    let lane = crate::native_v2_contract::RuntimeLane::new("opencode", "anthropic").assert_value();
    assert!(
        crate::harnesses::HarnessRegistry::local()
            .descriptor(&lane)
            .is_ok()
    );
    assert!(
        crate::harnesses::HarnessRegistry::core()
            .descriptor(&lane)
            .is_err()
    );
}

#[test]
fn declared_launch_controls_cannot_restore_native_managed_authority() {
    for name in [
        "OPENCODE_TEST_MANAGED_CONFIG_DIR",
        "opencode_test_managed_config_dir",
        "OPENCODE_CONFIG",
        "OPENCODE_PURE",
        "ProgramData",
        "pRoGrAmDaTa",
        "HOME",
        "USERPROFILE",
    ] {
        assert!(
            command::validate_connection_name(name, false).is_err(),
            "{name}"
        );
        assert!(
            command::validate_connection_name(name, true).is_err(),
            "{name}"
        );
    }
    for name in [
        "openai_api_key",
        "OpenAI_Base_Url",
        "copilot_github_token",
        "Aws_Secret_Access_Key",
        "Path",
        "tmpdir",
        "userprofile",
        "xDg_Config_Home",
    ] {
        assert!(
            command::validate_connection_name(name, true).is_err(),
            "{name}"
        );
    }
    for name in [
        "OPENAI_API_KEY",
        "ANTHROPIC_BASE_URL",
        "OPENCODE_PERMISSION",
        "CUSTOM_CONTEXT",
    ] {
        command::validate_connection_name(name, true).assert_value();
        command::validate_connection_name(name, false).assert_value();
    }
}

#[test]
fn contained_contribution_derives_explicit_auth_without_native_login_reuse() {
    use crate::native_v2_candidate::{ProviderAccessPlacement, materialize_provider_access};
    let lane = crate::native_v2_contract::RuntimeLane::new("opencode", "openai").assert_value();
    let binding = crate::native_v2_runner::test_support::binding(SessionScope::Execution);
    let mut runtime = crate::native_v2_contract::RuntimePlan::new(
        lane,
        openengine_cluster_protocol::RunSize::Medium,
        BTreeMap::from([(NodeName::new("worker").assert_value(), binding)]),
    );
    materialize_provider_access(&mut runtime, ProviderAccessPlacement::Contained).assert_value();
    let fields: Vec<_> = runtime
        .connection_requirements()
        .into_values()
        .flatten()
        .map(|field| field.to_string())
        .collect();
    assert_eq!(fields, vec!["OPENAI_API_KEY"]);
    assert!(
        crate::native_v2_hosting::materialize_contained_provider_access(&mut runtime).is_err(),
        "the default hosted factory must still refuse contributions"
    );
}

#[cfg(unix)]
mod process;

#[cfg(unix)]
#[path = "tests/native_authority.rs"]
mod native_authority;

#[cfg(unix)]
#[path = "tests/auth.rs"]
mod auth;

#[cfg(unix)]
#[path = "tests/providers.rs"]
mod providers;

#[cfg(unix)]
#[path = "tests/github.rs"]
mod github;
