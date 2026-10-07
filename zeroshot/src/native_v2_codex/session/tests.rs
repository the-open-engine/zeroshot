use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

fn usage(
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
) -> TokenUsageDelta {
    TokenUsageDelta {
        input_tokens: TokenCount::new(input_tokens).assert_value(),
        output_tokens: TokenCount::new(output_tokens).assert_value(),
        cache_read_input_tokens: cache_read_input_tokens
            .map(|value| TokenCount::new(value).assert_value()),
        cache_creation_input_tokens: cache_creation_input_tokens
            .map(|value| TokenCount::new(value).assert_value()),
    }
}

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

#[tokio::test]
async fn cumulative_usage_is_normalized_before_commit() {
    let session = CodexSession::new();
    let first = usage(10, 4, Some(3), Some(2));
    assert_eq!(session.usage_delta(Some(first), false).await, Some(first));
    session.commit_usage(Some(first), false).await;

    let second = usage(16, 9, Some(5), Some(7));
    assert_eq!(
        session.usage_delta(Some(second), true).await,
        Some(usage(6, 5, Some(2), Some(5)))
    );
    assert_eq!(session.usage_delta(None, true).await, None);
}

#[tokio::test]
async fn resets_and_fresh_turns_start_new_usage_generations() {
    let session = CodexSession::new();
    session
        .commit_usage(Some(usage(10, 4, Some(3), Some(2))), false)
        .await;

    for (resumed, observed) in [
        (true, usage(2, 1, Some(1), Some(1))),
        (false, usage(16, 9, Some(5), Some(7))),
    ] {
        assert_eq!(
            session.usage_delta(Some(observed), resumed).await,
            Some(observed)
        );
    }
}

#[tokio::test]
async fn missing_usage_preserves_only_a_resumed_threads_baseline() {
    for resumed in [false, true] {
        let session = CodexSession::new();
        session
            .commit_usage(Some(usage(13, 5, Some(4), Some(2))), false)
            .await;
        assert_eq!(session.usage_delta(None, resumed).await, None);
        session.commit_usage(None, resumed).await;

        let observed = usage(20, 8, Some(7), Some(6));
        let expected = if resumed {
            usage(7, 3, Some(3), Some(4))
        } else {
            observed
        };
        assert_eq!(
            session.usage_delta(Some(observed), true).await,
            Some(expected)
        );
    }
}
