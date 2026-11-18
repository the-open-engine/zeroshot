use super::*;

#[tokio::test]
async fn tool_activity_keeps_native_identity_status_and_bounded_redacted_payloads() {
    let fixture = Fixture::new("tool_activity").await;
    let events = verified_events(&fixture).await;
    let logs = events
        .iter()
        .filter_map(|event| match event {
            DurableNodeEvent::Output { output, .. } if output.text.starts_with("Copilot tool ") => {
                Some(&output.text)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(logs.len(), 6);
    for log in &logs {
        assert!(log.len() <= 8192);
        for expected in ["native-shell", "HEAD", "TAIL", "[truncated]", "[REDACTED]"] {
            assert!(log.contains(expected), "missing {expected}");
        }
        assert!(!log.contains("gho_fake-secret"));
        assert!(!log.contains("cumulative"));
    }
    for (id, status) in [("ok", "success"), ("bad", "error"), ("unknown", "unknown")] {
        assert!(
            logs.iter()
                .any(|log| log.contains(&format!("id={id} status={status}")))
        );
    }
}
