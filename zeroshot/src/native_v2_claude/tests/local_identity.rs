use super::*;
use crate::native_v2_runner::ResolvedEnvironment;

fn local_anthropic_config(directory: &TestDirectory) -> ClaudeAdapterConfig {
    ClaudeAdapterConfig {
        provider: ClaudeProvider::Anthropic,
        executable: "claude".to_owned(),
        prefix_arguments: Vec::new(),
        workspace: directory.path().to_owned(),
        runtime_home: directory.child("runtime"),
        local_user_home: Some(directory.child("home")),
        native_environment: Default::default(),
        base_environment: ClaudeProcessEnvironment::default(),
        process_pool: HostedProcessPool::new(10_002, 10_002, 20_000, 20_000).assert_value(),
    }
}

#[test]
fn local_claude_user_reuses_home_without_moving_session_state() {
    let directory = TestDirectory::new("claude-local-user");
    let runtime_home = directory.child("runtime");
    let local_home = directory.child("home");
    let adapter = ClaudeAdapter::new_for_test(ClaudeAdapterConfig {
        provider: ClaudeProvider::Anthropic,
        executable: "claude".to_owned(),
        prefix_arguments: Vec::new(),
        workspace: directory.path().to_owned(),
        runtime_home,
        local_user_home: Some(local_home.clone()),
        native_environment: Default::default(),
        base_environment: ClaudeProcessEnvironment::new(BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("TMPDIR".to_owned(), "/shared/tmp".to_owned()),
        ]))
        .assert_value(),
        process_pool: HostedProcessPool::new(10_002, 10_002, 20_000, 20_000).assert_value(),
    })
    .assert_value();
    let binding = agent_binding(
        "claude-sonnet-5",
        Some(ReasoningEffort::Max),
        SessionScope::Execution,
        &[],
    );
    let environment = ResolvedEnvironment::exact(&binding, BTreeMap::new()).assert_value();
    let values = adapter
        .process_environment(&environment, Path::new("/private/session"))
        .assert_value();

    assert_eq!(values.get("HOME").map(String::as_str), local_home.to_str());
    assert_eq!(
        values.get("TMPDIR").map(String::as_str),
        Some("/private/session")
    );
}

#[test]
fn claude_connections_override_local_configuration_defaults() {
    let directory = TestDirectory::new("claude-local-overrides");
    let configuration = || local_anthropic_config(&directory);
    let mut adapter = ClaudeAdapter::new_for_test(configuration()).assert_value();
    adapter.local_environment = BTreeMap::from([
        (
            "ANTHROPIC_BASE_URL".to_owned(),
            "https://shell.example/private".to_owned(),
        ),
        ("CLAUDE_CONFIG_DIR".to_owned(), "/shell/config".to_owned()),
    ]);
    let binding = agent_binding(
        "proxy-model",
        None,
        SessionScope::Execution,
        &["ANTHROPIC_BASE_URL"],
    );
    let resolved = ResolvedEnvironment::exact(
        &binding,
        BTreeMap::from([(
            environment_name("ANTHROPIC_BASE_URL"),
            "https://connection.example/private".to_owned(),
        )]),
    )
    .assert_value();
    let values = adapter
        .process_environment(&resolved, Path::new("/private/session"))
        .assert_value();
    assert_eq!(
        values["ANTHROPIC_BASE_URL"],
        "https://connection.example/private"
    );
    assert_eq!(values["CLAUDE_CONFIG_DIR"], "/shell/config");
    let hosted = ClaudeAdapter::new(configuration()).assert_value();
    assert!(hosted.local_environment.is_empty());
    let values = hosted
        .process_environment(&resolved, Path::new("/private/session"))
        .assert_value();
    assert_eq!(
        values["ANTHROPIC_BASE_URL"],
        "https://connection.example/private"
    );
    assert_eq!(values["HOME"], "/private/session");
    assert!(!values.contains_key("CLAUDE_CONFIG_DIR"));
}

#[test]
fn declared_claude_auth_family_suppresses_every_ambient_alias() {
    let directory = TestDirectory::new("claude-auth-precedence");
    let configuration = local_anthropic_config(&directory);
    let mut adapter = ClaudeAdapter::new_for_test(configuration).assert_value();
    adapter.local_environment = BTreeMap::from([
        ("ANTHROPIC_API_KEY".to_owned(), "ambient-api".to_owned()),
        ("ANTHROPIC_AUTH_TOKEN".to_owned(), "ambient-auth".to_owned()),
        (
            "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
            "ambient-oauth".to_owned(),
        ),
        (
            "CLAUDE_CODE_OAUTH_REFRESH_TOKEN".to_owned(),
            "ambient-refresh".to_owned(),
        ),
    ]);
    let binding = agent_binding(
        "claude-sonnet-5",
        None,
        SessionScope::Execution,
        &["ANTHROPIC_API_KEY"],
    );
    let environment = ResolvedEnvironment::exact(
        &binding,
        BTreeMap::from([(
            environment_name("ANTHROPIC_API_KEY"),
            "declared-key".to_owned(),
        )]),
    )
    .assert_value();
    let values = adapter
        .process_environment(&environment, Path::new("/private/session"))
        .assert_value();
    assert_eq!(values["ANTHROPIC_API_KEY"], "declared-key");
    for name in [
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_REFRESH_TOKEN",
    ] {
        assert!(!values.contains_key(name));
    }
}
