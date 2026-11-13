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
        "#!/bin/sh\nif [ \"$1\" = auth ] && [ \"$2\" = export ]; then echo \"$XDG_DATA_HOME\" >> export-env; {export_body}; exit 0; fi\nif [ \"$1\" = auth ] && [ \"$2\" = import ]; then echo \"$XDG_DATA_HOME\" >> import-env; cat >> import-stdin; exit 0; fi\necho \"$XDG_DATA_HOME\" >> run-env\ncat >/dev/null\nprintf '%s\\n' '{text}' '{finish}'\n"
    )
}

#[tokio::test]
async fn a_local_run_imports_only_the_lane_login_into_its_private_store() {
    let directory = TestDirectory::new("opencode-store-import");
    let adapter = adapter(&directory, &stub(NORMAL_EXPORT));
    assert!(run(&adapter, invocation(&[])).await.is_ok());
    let imported =
        std::fs::read_to_string(adapter.config.workspace.join("import-stdin")).assert_value();
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
