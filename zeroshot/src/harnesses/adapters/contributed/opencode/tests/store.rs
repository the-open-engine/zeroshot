use super::process::{adapter, invocation, run};
use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_contract::NodeInstanceId;
use openengine_cluster_testkit::assertions::AssertError;
use serde_json::Value;

const NORMAL_EXPORT: &str = r#"printf '%s\n' '[{"id":"cred_a","integrationID":"anthropic","label":"a","active":true,"value":{"type":"key","key":"stored-anthropic-secret"}},{"id":"cred_o","integrationID":"openai","label":"o","active":true,"value":{"type":"key","key":"stored-openai-secret"}}]'"#;

fn stub(export_body: &str) -> String {
    let text = text_event("t1", "m1", "{\"response\":null}").to_string();
    let finish = finish_event("f1", "m1", "stop").to_string();
    format!(
        "#!/bin/sh\n\
         if [ \"$1\" = auth ] && [ \"$2\" = export ]; then echo \"$XDG_DATA_HOME\" >> export-env; \
         echo \"$*\" >> export-args; printf '%s\\n' \"$OPENCODE_CONFIG_CONTENT\" >> export-config; \
         {LOADS_PLUGINS}; {export_body}; exit 0; fi\n\
         if [ \"$1\" = auth ] && [ \"$2\" = import ]; then echo \"$XDG_DATA_HOME\" >> import-env; \
         printf '%s\\n' \"$OPENCODE_CONFIG_CONTENT\" >> import-config; cat >> import-stdin; \
         printf '\\n' >> import-stdin; exit 0; fi\n\
         if [ \"$1\" = auth ] && [ \"$2\" = logout ]; then echo \"$XDG_DATA_HOME\" >> logout-env; \
         echo \"$*\" >> logout-args; exit 0; fi\n\
         echo \"$XDG_DATA_HOME\" >> run-env\n\
         {LOADS_PLUGINS}\n\
         cat >/dev/null\n\
         printf '%s\\n' '{text}' '{finish}'\n"
    )
}

/// Each import appends one JSON array line to `import-stdin`.
fn imports(adapter: &OpenCodeAdapter) -> Vec<Vec<Value>> {
    std::fs::read_to_string(adapter.config.workspace.join("import-stdin"))
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).assert_value())
        .collect()
}

#[tokio::test]
async fn a_local_run_imports_only_the_lane_login_into_its_private_store() {
    let directory = TestDirectory::new("opencode-store-import");
    let adapter = adapter(&directory, &stub(NORMAL_EXPORT));
    assert!(run(&adapter, invocation(&[])).await.is_ok());
    let imported = std::fs::read_to_string(adapter.config.workspace.join("import-stdin"))
        .assert_value()
        .trim()
        .to_owned();
    assert!(!imported.contains("stored-openai-secret"));
    let entries: Vec<Value> = serde_json::from_str(&imported).assert_value();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].get("integrationID").and_then(Value::as_str),
        Some("anthropic")
    );
    let run_env = std::fs::read_to_string(adapter.config.workspace.join("run-env")).assert_value();
    let import_env =
        std::fs::read_to_string(adapter.config.workspace.join("import-env")).assert_value();
    let export_env =
        std::fs::read_to_string(adapter.config.workspace.join("export-env")).assert_value();
    let runtime_home = adapter.config.runtime_home.to_string_lossy().into_owned();
    let run_lines: Vec<&str> = run_env.lines().collect();
    let import_lines: Vec<&str> = import_env.lines().collect();
    let export_lines: Vec<&str> = export_env.lines().collect();
    assert_eq!(run_lines.len(), 1);
    assert_eq!(import_lines.len(), 1);
    assert_eq!(run_lines[0], import_lines[0]);
    assert!(run_lines[0].starts_with(&runtime_home));
    assert_eq!(export_lines.len(), 1);
    assert!(!export_lines[0].starts_with(&runtime_home));
}

#[tokio::test]
async fn a_declared_credential_skips_the_saved_login() {
    let directory = TestDirectory::new("opencode-store-declared");
    let adapter = adapter(&directory, &stub(NORMAL_EXPORT));
    assert!(
        run(
            &adapter,
            invocation(&[("ANTHROPIC_API_KEY", "declared-key")])
        )
        .await
        .is_ok()
    );
    assert!(!adapter.config.workspace.join("export-env").exists());
    assert!(!adapter.config.workspace.join("import-stdin").exists());
    assert!(adapter.config.workspace.join("run-env").exists());
}

#[tokio::test]
async fn the_login_moves_once_per_session() {
    let directory = TestDirectory::new("opencode-store-once");
    let adapter = adapter(&directory, &stub(NORMAL_EXPORT));
    let invocation = invocation(&[]);
    assert!(run(&adapter, invocation.clone()).await.is_ok());
    assert!(run(&adapter, invocation.clone()).await.is_ok());
    let export_env =
        std::fs::read_to_string(adapter.config.workspace.join("export-env")).assert_value();
    let run_env = std::fs::read_to_string(adapter.config.workspace.join("run-env")).assert_value();
    let export_lines: Vec<&str> = export_env.lines().collect();
    let run_lines: Vec<&str> = run_env.lines().collect();
    assert_eq!(export_lines.len(), 1);
    assert_eq!(run_lines.len(), 2);
    assert_eq!(run_lines[0], run_lines[1]);
}

#[tokio::test]
async fn two_sessions_never_share_a_store() {
    let directory = TestDirectory::new("opencode-store-sessions");
    let adapter = adapter(&directory, &stub(NORMAL_EXPORT));
    let first = invocation(&[]);
    let mut second = invocation(&[]);
    second.provider_session_slot = NodeInstanceId::new(2).assert_value();
    second.provider_home_slot = NodeInstanceId::new(2).assert_value();
    assert!(run(&adapter, first).await.is_ok());
    assert!(run(&adapter, second).await.is_ok());
    let run_env = std::fs::read_to_string(adapter.config.workspace.join("run-env")).assert_value();
    let run_lines: Vec<&str> = run_env.lines().collect();
    assert_eq!(run_lines.len(), 2);
    assert_ne!(run_lines[0], run_lines[1]);
    assert!(!run_lines[0].starts_with(run_lines[1]));
    assert!(!run_lines[1].starts_with(run_lines[0]));
}

#[tokio::test]
async fn a_failed_export_fails_the_node_without_its_output() {
    let directory = TestDirectory::new("opencode-store-failed-export");
    let adapter = adapter(
        &directory,
        &stub(r#"printf '%s' '[{"value":{"key":"leaked-secret"}}]'; exit 3"#),
    );
    let error = run(&adapter, invocation(&[])).await.assert_error();
    let message = error.to_string();
    assert!(message.contains("saved login"));
    assert!(!message.contains("leaked-secret"));
    assert!(!adapter.config.workspace.join("run-env").exists());
}

#[tokio::test]
async fn a_store_launch_keeps_unconfirmed_cleanup_and_hides_launch_details() {
    use crate::execution::process::ProcessRunnerError;
    use crate::native_v2_runner::NodeRunnerError;
    use super::super::store::launched;

    let (cancel, control, _durable) = crate::native_v2_runner::test_support::control();
    assert_eq!(launched(Ok(Ok(7)), &control, "failure"), Ok(7));
    for fatal in [
        NodeRunnerError::CleanupUnconfirmed,
        NodeRunnerError::Cancelled,
    ] {
        assert_eq!(
            launched::<u8>(Err(fatal.clone()), &control, "failure"),
            Err(fatal)
        );
    }
    let refused = Ok(Err(ProcessRunnerError::Launch("leaked-secret".to_owned())));
    assert_eq!(
        launched::<u8>(refused, &control, "failure"),
        Err(NodeRunnerError::DriverDetail("failure".to_owned()))
    );
    cancel.send(true).assert_value();
    let refused = Ok(Err(ProcessRunnerError::Launch("leaked-secret".to_owned())));
    assert_eq!(
        launched::<u8>(refused, &control, "failure"),
        Err(NodeRunnerError::Cancelled)
    );
}

const TWO_LOGINS: &str = r#"printf '%s\n' '[{"id":"cred_old","integrationID":"anthropic","label":"old","active":false,"value":{"type":"key","key":"inactive-anthropic-secret"}},{"id":"cred_new","integrationID":"anthropic","label":"new","active":true,"value":{"type":"key","key":"active-anthropic-secret"}}]'"#;

#[tokio::test]
async fn the_export_asks_only_for_the_lane_integration() {
    let directory = TestDirectory::new("opencode-store-export-target");
    let adapter = adapter(&directory, &stub(NORMAL_EXPORT));
    assert!(run(&adapter, invocation(&[])).await.is_ok());
    let arguments =
        std::fs::read_to_string(adapter.config.workspace.join("export-args")).assert_value();
    let lines: Vec<&str> = arguments.lines().collect();
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].ends_with("auth export --standalone anthropic"),
        "{arguments}"
    );
}

#[tokio::test]
async fn only_the_active_login_of_the_integration_moves() {
    let directory = TestDirectory::new("opencode-store-active-login");
    let adapter = adapter(&directory, &stub(TWO_LOGINS));
    assert!(run(&adapter, invocation(&[])).await.is_ok());
    let imported = std::fs::read_to_string(adapter.config.workspace.join("import-stdin"))
        .assert_value()
        .trim()
        .to_owned();
    assert!(
        !imported.contains("inactive-anthropic-secret"),
        "{imported}"
    );
    let entries: Vec<Value> = serde_json::from_str(&imported).assert_value();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].get("id").and_then(Value::as_str),
        Some("cred_new")
    );
}

#[tokio::test]
async fn an_integration_without_an_active_login_imports_nothing() {
    let directory = TestDirectory::new("opencode-store-no-active-login");
    let adapter = adapter(
        &directory,
        &stub(
            r#"printf '%s\n' '[{"id":"cred_old","integrationID":"anthropic","label":"old","active":false,"value":{"type":"key","key":"inactive-anthropic-secret"}}]'"#,
        ),
    );
    assert!(run(&adapter, invocation(&[])).await.is_ok());
    assert!(!adapter.config.workspace.join("import-stdin").exists());
    assert!(adapter.config.workspace.join("run-env").exists());
}

const OAUTH_EXPORT: &str = r#"printf '%s\n' '[{"id":"cred_oauth","integrationID":"anthropic","label":"oauth","active":true,"value":{"type":"oauth","methodID":"browser","refresh":"refresh-secret-one","access":"access-secret-one","expires":1999999999999,"metadata":{"accountID":"account-one"}}}]'"#;

#[tokio::test]
async fn the_renewal_plugin_loads_only_in_the_user_profile() {
    let directory = TestDirectory::new("opencode-store-renewal-plugin");
    let adapter = adapter(&directory, &stub(OAUTH_EXPORT));
    assert!(run(&adapter, invocation(&[])).await.is_ok());
    let export_config =
        std::fs::read_to_string(adapter.config.workspace.join("export-config")).assert_value();
    let import_config =
        std::fs::read_to_string(adapter.config.workspace.join("import-config")).assert_value();
    assert!(
        export_config.contains("/zeroshot-plugins/zeroshot-login-renewal/"),
        "{export_config}"
    );
    assert!(
        !import_config.contains("zeroshot-login-renewal"),
        "{import_config}"
    );
}

#[tokio::test]
async fn a_renewal_that_did_not_load_fails_the_node() {
    let directory = TestDirectory::new("opencode-store-renewal-missing");
    let body = stub(OAUTH_EXPORT).replacen(&format!("{LOADS_PLUGINS}; "), "", 1);
    let adapter = adapter(&directory, &body);
    let error = run(&adapter, invocation(&[])).await.err().assert_value();
    let message = error.to_string();
    assert!(
        message.contains("could not renew the saved login in the user's profile"),
        "{message}"
    );
    assert!(!message.contains("secret-one"), "{message}");
    assert!(!adapter.config.workspace.join("import-stdin").exists());
    assert!(!adapter.config.workspace.join("run-env").exists());
}

#[tokio::test]
async fn an_oauth_login_with_a_lifetime_moves_without_its_refresh_token() {
    let directory = TestDirectory::new("opencode-store-oauth-access-only");
    let adapter = adapter(&directory, &stub(OAUTH_EXPORT));
    assert!(run(&adapter, invocation(&[])).await.is_ok());
    let imports = imports(&adapter);
    assert_eq!(imports.len(), 1);
    let text = serde_json::to_string(&imports[0]).assert_value();
    assert!(!text.contains("refresh-secret-one"), "{text}");
    let value = &imports[0][0]["value"];
    assert_eq!(value["refresh"], "");
    assert_eq!(value["access"], "access-secret-one");
    assert_eq!(value["expires"], 0);
    assert_eq!(value["methodID"], "browser");
    assert_eq!(value["metadata"]["accountID"], "account-one");
}

#[tokio::test]
async fn an_oauth_login_without_a_lifetime_moves_unchanged() {
    let directory = TestDirectory::new("opencode-store-oauth-no-lifetime");
    let body = OAUTH_EXPORT.replace("1999999999999", "0");
    let adapter = adapter(&directory, &stub(&body));
    let invocation = invocation(&[]);
    assert!(run(&adapter, invocation.clone()).await.is_ok());
    assert!(run(&adapter, invocation).await.is_ok());
    let imports = imports(&adapter);
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0][0]["value"]["refresh"], "refresh-secret-one");
    let exports =
        std::fs::read_to_string(adapter.config.workspace.join("export-args")).assert_value();
    assert_eq!(exports.lines().count(), 1);
}

#[tokio::test]
async fn a_renewed_login_replaces_the_private_copy_on_the_next_turn() {
    let directory = TestDirectory::new("opencode-store-oauth-renewed");
    let renewed = OAUTH_EXPORT
        .replace("access-secret-one", "access-secret-two")
        .replace("refresh-secret-one", "refresh-secret-two");
    let body = format!(
        "if [ -f exported-once ]; then {renewed}; else : > exported-once; {OAUTH_EXPORT}; fi"
    );
    let adapter = adapter(&directory, &stub(&body));
    let invocation = invocation(&[]);
    assert!(run(&adapter, invocation.clone()).await.is_ok());
    assert!(run(&adapter, invocation).await.is_ok());
    let exports =
        std::fs::read_to_string(adapter.config.workspace.join("export-args")).assert_value();
    assert_eq!(exports.lines().count(), 2);
    let logouts =
        std::fs::read_to_string(adapter.config.workspace.join("logout-args")).assert_value();
    let logout_lines: Vec<&str> = logouts.lines().collect();
    assert_eq!(logout_lines.len(), 1);
    assert!(
        logout_lines[0].ends_with("auth logout --standalone anthropic cred_oauth"),
        "{logouts}"
    );
    let logout_env =
        std::fs::read_to_string(adapter.config.workspace.join("logout-env")).assert_value();
    let import_env =
        std::fs::read_to_string(adapter.config.workspace.join("import-env")).assert_value();
    assert_eq!(logout_env.lines().next(), import_env.lines().next());
    let imports = imports(&adapter);
    assert_eq!(imports.len(), 2);
    assert_eq!(imports[0][0]["value"]["access"], "access-secret-one");
    assert_eq!(imports[1][0]["value"]["access"], "access-secret-two");
    assert_eq!(imports[1][0]["value"]["refresh"], "");
}

#[tokio::test]
async fn an_unchanged_login_is_not_imported_again() {
    let directory = TestDirectory::new("opencode-store-oauth-unchanged");
    let adapter = adapter(&directory, &stub(OAUTH_EXPORT));
    let invocation = invocation(&[]);
    assert!(run(&adapter, invocation.clone()).await.is_ok());
    assert!(run(&adapter, invocation).await.is_ok());
    let exports =
        std::fs::read_to_string(adapter.config.workspace.join("export-args")).assert_value();
    assert_eq!(exports.lines().count(), 2);
    assert_eq!(imports(&adapter).len(), 1);
    assert!(!adapter.config.workspace.join("logout-args").exists());
}
