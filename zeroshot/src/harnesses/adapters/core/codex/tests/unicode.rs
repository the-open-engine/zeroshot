use super::*;

#[tokio::test]
async fn decoded_unicode_is_redacted_from_agent_messages() {
    let directory = TestDirectory::new("codex-unicode-redaction");
    let adapter = scripted_adapter_with(
        &directory,
        CodexProvider::OpenAi,
        "codex-unicode",
        r#"#!/bin/sh
set -eu
/usr/bin/cat >/dev/null
/usr/bin/printf '%s\n' '{"type":"thread.started","thread_id":"unicode-thread"}'
/usr/bin/printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"visible \u0074oken-\uD83D-end"}}'
/usr/bin/printf '%s' '{"type":"item.completed","item":{"type":"agent_message","text":"visible \u0074oken-'
/usr/bin/printf '\377'
/usr/bin/printf '%s\n' '-end"}}'
/usr/bin/printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"{\"response\":{\"answer\":42}}"}}'
/usr/bin/printf '%s\n' '{"type":"turn.completed"}'
"#,
    );
    let admitted = admitted(
        binding(SessionScope::Execution, &["OPENAI_API_KEY", "TEST_SECRET"]),
        CodexProvider::OpenAi,
    )
    .await;
    let runtime = runner(&admitted, adapter);
    let handle = start(
        &runtime,
        &admitted,
        1,
        &[
            ("OPENAI_API_KEY", "fake-openai-key".to_owned()),
            ("TEST_SECRET", "token-�-end".to_owned()),
        ],
    )
    .await;
    let logs = complete_verified_with_logs(handle).await;
    assert_eq!(logs.matches("visible [REDACTED]").count(), 2);
    assert!(!logs.contains("token-"));
    assert!(!logs.contains('�'));
}
