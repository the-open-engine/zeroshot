use super::*;

#[tokio::test]
async fn thread_ids_are_opaque_beyond_process_argv_requirements() {
    let session = CodexSession::new();
    let thread_id = format!("thread-{}\nwith-tab\t", "x".repeat(512));

    assert_eq!(session.record_thread(Some(&thread_id), None).await, Ok(()));
    assert_eq!(
        session
            .record_thread(Some(&thread_id), Some(&thread_id))
            .await,
        Ok(())
    );
    assert_eq!(
        session.thread_id.lock().await.as_deref(),
        Some(thread_id.as_str())
    );
}

#[tokio::test]
async fn thread_ids_reject_only_missing_empty_nul_or_conflicting_values() {
    for (observed, expected) in [
        (None, "Codex output did not provide a thread ID"),
        (Some(""), "Codex output provided an empty thread ID"),
        (
            Some("thread\0id"),
            "Codex output thread ID contained a NUL byte",
        ),
    ] {
        assert_eq!(
            CodexSession::new().record_thread(observed, None).await,
            Err(expected)
        );
    }

    let session = CodexSession::new();
    assert_eq!(session.record_thread(Some("one"), None).await, Ok(()));
    assert_eq!(
        session.record_thread(Some("two"), Some("one")).await,
        Err("Codex output thread ID did not match the resumed session")
    );
    assert_eq!(
        session.record_thread(None, Some("two")).await,
        Err("Codex output thread ID changed across turns")
    );
}
