//! Configuration-boundary tests for the Pi adapter.
//!
//! These cover the checks the adapter performs before a turn is dispatched: the capsule
//! environment it hands a contained child, the empty-executable and empty-runtime-home guards, and
//! the session factory and identity checks that reject a non-agent binding.
//!
//! A child module of `native_v2_pi/tests.rs`, so the shared fixtures stay in one place and the
//! coverage totals keep excluding this file (`AGENTS.md`).

use super::*;
use serde_json::json;

#[test]
fn contained_native_aliases_cannot_replace_host_tools_or_project_trust() {
    let directory = TestDirectory::new("pi-contained-native-aliases");
    let files = files(&directory);
    let adapter = PiAdapter::new(adapter_configuration(
        PiProvider::Anthropic,
        "pi",
        BTreeMap::new(),
    ))
    .assert_value();
    for args in [
        vec!["-a"],
        vec!["--approve"],
        vec!["-t", "read"],
        vec!["--tools", "read"],
        vec!["-nt"],
        vec!["--no-tools"],
        vec!["-nbt"],
        vec!["--no-builtin-tools"],
        vec!["-xt", "read"],
        vec!["--exclude-tools", "read"],
        vec!["-na"],
        vec!["--no-approve"],
    ] {
        let mut turn = invocation(
            agent_binding(
                "opaque",
                None,
                SessionScope::Execution,
                &["ANTHROPIC_API_KEY"],
            ),
            NodeRole::Worker,
            resolved(
                &["ANTHROPIC_API_KEY"],
                &[("ANTHROPIC_API_KEY", "fixture-key")],
            ),
        );
        crate::harnesses::native_config::tests::set(&mut turn.node.binding, json!({"args":args}));
        let error = adapter
            .command_for_test(&turn, &files, "zs-owned-session")
            .unwrap_err();
        let detail = error.to_string();
        for expected in [
            "node `agent`",
            "harness `pi`",
            "harnessConfig.args",
            args[0],
        ] {
            assert!(detail.contains(expected), "{detail}");
        }
    }
}

#[test]
fn local_native_tool_modifiers_reach_the_pi_command_unchanged() {
    let directory = TestDirectory::new("pi-native-tool-modifiers");
    let files = files(&directory);
    let adapter = PiAdapter::new_local(adapter_configuration(
        PiProvider::Anthropic,
        "pi",
        BTreeMap::new(),
    ))
    .assert_value();
    for args in [vec!["--tools", "-read"], vec!["-t", "-read,-bash"]] {
        let mut turn = invocation(
            agent_binding("opaque", None, SessionScope::Execution, &[]),
            NodeRole::Worker,
            resolved(&[], &[]),
        );
        crate::harnesses::native_config::tests::set(&mut turn.node.binding, json!({"args":args}));
        let command = adapter
            .command_for_test(&turn, &files, "zs-owned-session")
            .assert_value();
        assert_eq!(&command.argv[command.argv.len() - args.len()..], &args);
        assert_eq!(
            value(&command.argv, "--session-id").as_deref(),
            Some("zs-owned-session")
        );
    }
}

#[test]
fn native_fork_is_rejected_before_launch_in_both_placements() {
    let directory = TestDirectory::new("pi-owned-session-history");
    let files = files(&directory);
    for contained in [false, true] {
        let config = adapter_configuration(PiProvider::Anthropic, "pi", BTreeMap::new());
        let adapter = if contained {
            PiAdapter::new(config)
        } else {
            PiAdapter::new_local(config)
        }
        .assert_value();
        let mut turn = invocation(
            agent_binding(
                "opaque",
                None,
                SessionScope::NodeInstance,
                &["ANTHROPIC_API_KEY"],
            ),
            NodeRole::Worker,
            resolved(
                &["ANTHROPIC_API_KEY"],
                &[("ANTHROPIC_API_KEY", "fixture-key")],
            ),
        );
        crate::harnesses::native_config::tests::set(
            &mut turn.node.binding,
            json!({"args":["--fork","caller-history.jsonl"]}),
        );
        let detail = adapter
            .command_for_test(&turn, &files, "zs-owned-session")
            .unwrap_err()
            .to_string();
        assert!(
            detail.contains("--fork") && detail.contains("harnessConfig.args"),
            "{detail}"
        );
    }
}

#[test]
fn coverage_contract_pi_capsule_environment_supplies_a_home_and_default_search_path() {
    let environment =
        PiProcessEnvironment::new(BTreeMap::from([("LANG".to_owned(), "C".to_owned())]))
            .assert_value();
    let capsule = environment
        .for_capsule(Path::new("/run/pi-home"), "/usr/bin:/bin")
        .assert_value();
    let values = capsule.clone_values();
    // The capsule owns the child's home; the run supplies the executable search path.
    assert_eq!(values.get("HOME").map(String::as_str), Some("/run/pi-home"));
    assert_eq!(
        values.get("PATH").map(String::as_str),
        Some("/usr/bin:/bin")
    );
    assert_eq!(values.get("LANG").map(String::as_str), Some("C"));
}

#[test]
fn coverage_contract_pi_capsule_environment_rejects_an_empty_runtime_home() {
    let environment =
        PiProcessEnvironment::new(BTreeMap::from([("PATH".to_owned(), "/bin".to_owned())]))
            .assert_value();
    assert!(matches!(
        environment.for_capsule(Path::new(""), "/bin"),
        Err(crate::native_v2_capsule::ProcessEnvironmentError::InvalidEnvironment)
    ));
}

#[test]
fn coverage_contract_pi_hosted_configuration_rejects_an_empty_executable() {
    assert!(matches!(
        PiAdapter::new(adapter_configuration(
            PiProvider::Anthropic,
            "",
            BTreeMap::new(),
        )),
        Err(PiAdapterConfigError::EmptyExecutable)
    ));
}

#[test]
fn coverage_contract_pi_command_rejects_an_empty_runtime_home() {
    let directory = TestDirectory::new("pi-empty-runtime-home");
    let files = files(&directory);
    let mut configuration = adapter_configuration(PiProvider::Anthropic, "pi", BTreeMap::new());
    configuration.runtime_home = PathBuf::new();
    let adapter = PiAdapter::new_local(configuration).assert_value();
    let outcome = adapter.command_for_test(
        &invocation(
            agent_binding("claude-sonnet-4-5", None, SessionScope::Execution, &[]),
            NodeRole::Worker,
            resolved(&[], &[]),
        ),
        &files,
        "zs-fixed",
    );
    assert!(
        matches!(
            &outcome,
            Err(crate::native_v2_runner::NodeRunnerError::DriverDetail(detail))
                if detail.contains("runtime home")
        ),
        "{outcome:?}"
    );
}

#[test]
fn coverage_contract_pi_workspace_policy_rejects_a_delivery_role() {
    // A delivery role has no read-write workspace access, so the adapter must refuse the turn even
    // when the binding itself claims to be an agent.
    let directory = TestDirectory::new("pi-delivery-role");
    let files = files(&directory);
    let adapter = PiAdapter::new_local(adapter_configuration(
        PiProvider::Anthropic,
        "pi",
        BTreeMap::new(),
    ))
    .assert_value();
    let outcome = adapter.command_for_test(
        &invocation(
            agent_binding("claude-sonnet-4-5", None, SessionScope::Execution, &[]),
            NodeRole::GitDelivery,
            resolved(&[], &[]),
        ),
        &files,
        "zs-fixed",
    );
    assert!(outcome.is_err(), "a delivery role must not run a Pi turn");
}

#[tokio::test]
async fn coverage_contract_pi_session_open_accepts_an_agent_binding() {
    use crate::native_v2_runner::SessionFactory;

    let adapter = PiAdapter::new(adapter_configuration(
        PiProvider::Anthropic,
        "pi",
        BTreeMap::new(),
    ))
    .assert_value();
    let turn = invocation(
        agent_binding("claude-sonnet-4-5", None, SessionScope::Execution, &[]),
        NodeRole::Worker,
        resolved(&[], &[]),
    );
    assert!(
        SessionFactory::open(&adapter, &turn.node, &turn.environment)
            .await
            .is_ok(),
        "an agent binding opens a Pi session"
    );
}

#[tokio::test]
async fn coverage_contract_pi_session_open_rejects_a_non_agent_binding() {
    use crate::native_v2_runner::SessionFactory;

    let adapter = PiAdapter::new(adapter_configuration(
        PiProvider::Anthropic,
        "pi",
        BTreeMap::new(),
    ))
    .assert_value();
    let turn = invocation(delivery_binding(), NodeRole::Worker, resolved(&[], &[]));
    assert!(matches!(
        SessionFactory::open(&adapter, &turn.node, &turn.environment).await,
        Err(crate::native_v2_runner::NodeRunnerError::SessionOpen)
    ));
}

#[test]
fn coverage_contract_pi_session_identity_requires_an_agent_binding() {
    let turn = invocation(delivery_binding(), NodeRole::Worker, resolved(&[], &[]));
    assert!(matches!(
        identity(&turn),
        Err(crate::native_v2_runner::NodeRunnerError::Driver)
    ));
}

#[test]
fn coverage_contract_pi_prompt_requires_authored_instructions() {
    // The prompt renderer needs the authored instructions; a node without them must fail rather
    // than send a prompt that silently omits its task.
    let mut turn = invocation(
        agent_binding("claude-sonnet-4-5", None, SessionScope::Execution, &[]),
        NodeRole::Worker,
        resolved(&[], &[]),
    );
    turn.node.instructions = None;
    assert!(
        super::super::prompt(ProviderSchemaDialect::Standard, &turn).is_err(),
        "authored instructions are required"
    );
}

#[test]
fn coverage_contract_pi_lanes_without_native_login_accept_no_declared_credential() {
    // Only the Anthropic and OpenAI lanes reuse a stored Pi login; every other lane authenticates
    // from its declared connection, so it accepts no native-local credential field.
    assert!(super::super::command::native_local_credentials(PiProvider::Anthropic).len() == 3);
    assert!(super::super::command::native_local_credentials(PiProvider::OpenAi).len() == 1);
    assert!(super::super::command::native_local_credentials(PiProvider::OpenRouter).is_empty());
    assert!(super::super::command::native_local_credentials(PiProvider::Bedrock).is_empty());
    assert!(super::super::command::native_local_credentials(PiProvider::Gateway).is_empty());
}
