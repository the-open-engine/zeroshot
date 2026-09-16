use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn detached_controller_preserves_local_codex_configuration_through_correction() {
    exercise_local_configuration(
        "codex",
        HarnessCase {
            contents: CODEX_CONFIG,
            probe: ProbeResponse::Configured(
                json!({"sandbox_workspace_write":{"network_access":false}}),
            ),
            bypass: false,
            environment: None,
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn detached_controller_preserves_local_claude_configuration_through_correction() {
    exercise_local_configuration(
        "claude",
        HarnessCase {
            contents: CLAUDE_CONFIG,
            probe: ProbeResponse::Configured(json!({"env":{"LOCAL_PREFERENCE":"preserved"}})),
            bypass: true,
            environment: None,
        },
    )
    .await;
}

struct HarnessCase {
    contents: &'static str,
    probe: ProbeResponse,
    bypass: bool,
    environment: Option<PermissionEnvironment>,
}

struct PermissionEnvironment {
    name: &'static str,
    inherited: &'static str,
}

#[tokio::test(flavor = "multi_thread")]
async fn detached_controller_codex_permission_defaults_follow_configuration() {
    let explicit = json!({"sandbox_mode":"workspace-write","approval_policy":"on-request"});
    for case in permission_cases(
        "sandbox_mode = \"workspace-write\"\napproval_policy = \"on-request\"\n",
        explicit,
        "",
    ) {
        exercise_local_configuration("codex", case).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn detached_controller_claude_permission_defaults_follow_configuration() {
    let explicit = json!({"permissions":{"defaultMode":"plan","deny":["Write"]}});
    for case in permission_cases(
        r#"{"permissions":{"defaultMode":"plan","deny":["Write"]}}"#,
        explicit,
        "{}\n",
    ) {
        exercise_local_configuration("claude", case).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn detached_controller_preserves_claude_permission_environment() {
    for (name, inherited) in [
        ("CLAUDE_CODE_FORCE_SANDBOX", "true"),
        ("CLAUDE_CODE_RESTRICTED", "true"),
        ("CLAUDE_CODE_AUTO_MODE_EXTERNAL_PERMISSIONS", "true"),
        ("CLAUDE_BG_SESSION_PERMISSION_RULES", r#"{"deny":["Bash"]}"#),
    ] {
        exercise_local_configuration(
            "claude",
            HarnessCase {
                contents: "{}\n",
                probe: ProbeResponse::Configured(json!({})),
                bypass: false,
                environment: Some(PermissionEnvironment { name, inherited }),
            },
        )
        .await;
    }
}

fn permission_cases(
    contents: &'static str,
    explicit: Value,
    empty: &'static str,
) -> [HarnessCase; 4] {
    [
        HarnessCase {
            contents: empty,
            probe: ProbeResponse::Configured(json!({})),
            bypass: true,
            environment: None,
        },
        HarnessCase {
            contents,
            probe: ProbeResponse::Configured(explicit),
            bypass: false,
            environment: None,
        },
        HarnessCase {
            contents: empty,
            probe: ProbeResponse::Unsupported,
            bypass: false,
            environment: None,
        },
        HarnessCase {
            contents: "invalid configuration",
            probe: ProbeResponse::Invalid,
            bypass: false,
            environment: None,
        },
    ]
}

async fn exercise_local_configuration(harness: &str, case: HarnessCase) {
    let mut fixture = LocalFixture::new();
    let home = fixture.root.path("user-home");
    fixture.working_directory = fixture.repository.join("nested");
    let config_dir = fixture.working_directory.join("custom-config");
    fs::create_dir_all(&home).assert_value();
    fs::create_dir_all(&config_dir).assert_value();
    fixture
        .harness_environment
        .insert("HOME".to_owned(), home.display().to_string());
    let config = case.contents;
    let (config_name, script) = match harness {
        "codex" => {
            fixture.harness_environment.extend([
                ("CODEX_HOME".to_owned(), "./custom-config".to_owned()),
                (
                    "OPENAI_BASE_URL".to_owned(),
                    "http://localhost:4000/v1".to_owned(),
                ),
                (
                    "CODEX_BASE_URL".to_owned(),
                    "http://localhost:4001/v1".to_owned(),
                ),
                (
                    "OPENAI_API_BASE".to_owned(),
                    "http://localhost:4002/v1".to_owned(),
                ),
            ]);
            ("config.toml", CODEX_SCRIPT)
        }
        "claude" => {
            let mut runtime = local_runtime();
            runtime["harness"] = json!("claude");
            runtime["provider"] = json!("anthropic");
            runtime["nodes"]["worker"]["model"] = json!("proxy-model");
            runtime["nodes"]["worker"]["connections"] = json!({"anthropic":["ANTHROPIC_API_KEY"]});
            write_json(&fixture.runtime, &runtime);
            fixture.harness_environment.extend([
                ("CLAUDE_CONFIG_DIR".to_owned(), "./custom-config".to_owned()),
                (
                    "ANTHROPIC_BASE_URL".to_owned(),
                    "http://localhost:4000/anthropic".to_owned(),
                ),
                (
                    "ANTHROPIC_API_KEY".to_owned(),
                    "local-declared-key".to_owned(),
                ),
                ("CLAUDE_CODE_USE_GATEWAY".to_owned(), "1".to_owned()),
            ]);
            ("settings.json", CLAUDE_SCRIPT)
        }
        _ => unreachable!(),
    };
    let script = permission_environment_script(&mut fixture, script, case.environment.as_ref());
    let config_path = config_dir.join(config_name);
    fs::write(&config_path, config).assert_value();
    let executable = fixture.root.path("bin").join(harness);
    fs::write(
        &executable,
        with_configuration_probe(script.as_bytes(), harness, &case.probe),
    )
    .assert_value();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).assert_value();

    let run_id = fixture.submit_detached("finish").await;
    assert_succeeded(&fixture, &run_id).await;
    assert_eq!(fs::read_to_string(&config_path).assert_value(), config);
    assert_eq!(
        fs::read_to_string(fixture.repository.join("config-capture")).assert_value(),
        config.repeat(2)
    );
    let args = fs::read_to_string(fixture.repository.join("harness-args")).assert_value();
    assert_harness_arguments(harness, &args, case.bypass);
    wait_for_exit(fixture.ready_pid(&run_id)).await;
}

fn permission_environment_script(
    fixture: &mut LocalFixture,
    script: &str,
    environment: Option<&PermissionEnvironment>,
) -> String {
    let Some(environment) = environment else {
        return script.to_owned();
    };
    fixture.harness_environment.insert(
        environment.name.to_owned(),
        environment.inherited.to_owned(),
    );
    let assertion = format!(
        "set -eu\ntest \"${{{}-}}\" = {}",
        environment.name,
        shell_literal(environment.inherited)
    );
    script.replacen("set -eu", &assertion, 1)
}

fn assert_harness_arguments(harness: &str, args: &str, bypass: bool) {
    let resume = if harness == "codex" {
        "resume"
    } else {
        "--resume"
    };
    assert_eq!(args.lines().filter(|line| *line == resume).count(), 1);
    assert!(!args.contains("model_provider="));
    assert!(!args.contains("web_search="));
    assert!(!args.contains("sandbox_workspace_write.network_access="));
    assert!(!args.contains("--setting-sources"));
    let bypass_flag = if harness == "codex" {
        "--dangerously-bypass-approvals-and-sandbox"
    } else {
        "--dangerously-skip-permissions"
    };
    assert_eq!(
        args.lines().filter(|line| *line == bypass_flag).count(),
        if bypass { 2 } else { 0 },
        "{harness} arguments: {args}"
    );
    assert!(
        !args
            .lines()
            .any(|line| line == "--sandbox" || line == "--permission-mode")
    );
    assert!(!args.contains("approval_policy="));
    assert!(!args.contains("--safe-mode"));
    assert!(!args.contains("app-server"));
}

const CODEX_SCRIPT: &str = r#"#!/bin/sh
set -eu
test "$OPENAI_API_KEY" = local-declared-key
test "$CODEX_API_KEY" = local-declared-key
test "$OPENAI_BASE_URL" = http://localhost:4000/v1
test "$CODEX_BASE_URL" = http://localhost:4001/v1
test "$OPENAI_API_BASE" = http://localhost:4002/v1
test -z "${UNDECLARED_SECRET+x}"
cat "$CODEX_HOME/config.toml" >> config-capture
printf '%s\n' "$@" >> harness-args
cat > prompt-capture
printf '%s\n' '{"type":"thread.started","thread_id":"local-thread"}'
if test -f first-turn; then
  printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"response\":null}"}}'
else
  touch first-turn
  printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"\"invalid\""}}'
fi
printf '%s\n' '{"type":"turn.completed"}'
"#;

const CLAUDE_SCRIPT: &str = r#"#!/bin/sh
set -eu
test "$ANTHROPIC_API_KEY" = local-declared-key
test "$ANTHROPIC_BASE_URL" = http://localhost:4000/anthropic
test "$CLAUDE_CODE_USE_GATEWAY" = 1
test -z "${UNDECLARED_SECRET+x}"
test -z "${OPENAI_API_KEY+x}"
cat "$CLAUDE_CONFIG_DIR/settings.json" >> config-capture
printf '%s\n' "$@" >> harness-args
cat > prompt-capture
printf '%s\n' '{"type":"system","subtype":"init","session_id":"local-session"}'
if test -f first-turn; then
  printf '%s%s\n' '{"type":"result","subtype":"success","is_error":false,' \
    '"result":"{\"response\":null}","session_id":"local-session"}'
else
  touch first-turn
  printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"invalid","session_id":"local-session"}'
fi
"#;

async fn assert_succeeded(fixture: &LocalFixture, run_id: &str) {
    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        let status = fixture.json(&["status", run_id], "finish").await;
        if status["status"]["phase"] == "finished" {
            assert_eq!(
                status["status"]["terminalResult"]["status"], "succeeded",
                "{status}"
            );
            return;
        }
        assert!(Instant::now() < deadline, "run did not finish: {status}");
        sleep(Duration::from_millis(30)).await;
    }
}

const CODEX_CONFIG: &str = concat!(
    "model_provider = \"litellm\"\n",
    "web_search = \"live\"\n",
    "[sandbox_workspace_write]\nnetwork_access = false\n",
    "[model_providers.litellm]\nname = \"LiteLLM\"\n",
    "base_url = \"http://localhost:4000/v1\"\n",
    "env_key = \"OPENAI_API_KEY\"\nwire_api = \"responses\"\n",
);

const CLAUDE_CONFIG: &str = "{\"env\":{\"LOCAL_PREFERENCE\":\"preserved\"}}\n";
