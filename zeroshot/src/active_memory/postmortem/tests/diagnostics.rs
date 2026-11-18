use super::*;

#[tokio::test]
async fn role_startup_and_unanswered_provider_failures_keep_the_cause_without_primary_evidence() {
    use crate::v2_run_ledger::SafeLogStream;
    for before_start in [true, false] {
        let fixture = fixture(Default::default(), None, true).await;
        fixture
            .driver
            .fail_before_start
            .store(before_start, Ordering::SeqCst);
        fixture
            .driver
            .fail_without_answer
            .store(!before_start, Ordering::SeqCst);
        fixture.drive().await;
        let stored = fixture
            .ledger
            .snapshot_and_tail(&fixture.run_id, None)
            .await
            .assert_value();
        let errors: Vec<_> = stored
            .events
            .iter()
            .filter_map(|event| match &event.event {
                RunEvent::SafeLog {
                    stream: SafeLogStream::Error,
                    line,
                    diagnostic,
                    ..
                } => {
                    assert!(diagnostic.is_none());
                    Some(line.as_str())
                }
                _ => None,
            })
            .collect();
        let cause = if before_start {
            "native session initialization refused"
        } else {
            "provider stderr: model not available"
        };
        for role in super::super::plan::Role::ALL {
            assert!(
                errors
                    .iter()
                    .any(|line| line.contains(role.name()) && line.contains(cause)),
                "{errors:?}"
            );
        }
        assert_eq!(
            errors
                .iter()
                .filter(|line| line.contains("postmortem completed:"))
                .count(),
            1
        );
        assert!(stored.snapshot.failure_context.is_none());
    }
}

#[tokio::test]
async fn rejected_discovery_reports_the_gate_without_publishing_proposal_text() {
    for (field, replacement, diagnostic) in [
        ("node_id", json!("invented_node"), "unknown executed node"),
        (
            "references",
            json!(["node:worker", "repository:src/provider.rs"]),
            "observed main-run evidence for this node",
        ),
        (
            "references",
            json!(["hook:999", "repository:src/provider.rs"]),
            "invalid evidence references",
        ),
        (
            "identifiers",
            json!(["invented_identifier"]),
            "source does not verify",
        ),
        (
            "title",
            json!("private-proposal-marker".repeat(10)),
            "invalid lesson card: invalid active memory: title",
        ),
    ] {
        let fixture = fixture(Default::default(), None, true).await;
        fixture.substantial_exploration();
        let mut proposal = candidate("worker");
        // In the association case, the invented identifier must be the only association.
        if field == "identifiers" {
            proposal["files"] = json!([]);
            proposal["folders"] = json!([]);
        }
        proposal[field] = replacement;
        fixture.output("repository", json!({"lessons":[proposal]}));
        fixture.drive().await;
        assert!(fixture.lessons().await.is_empty());
        let tail = fixture
            .ledger
            .snapshot_and_tail(&fixture.run_id, None)
            .await
            .assert_value();
        let logs = tail
            .events
            .iter()
            .filter_map(|event| match &event.event {
                RunEvent::SafeLog { line, .. } => Some(line.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(logs.contains(diagnostic), "missing {diagnostic}: {logs}");
        assert!(!logs.contains("private-proposal-marker"));
        assert!(logs.contains("3 roles completed, 1 failed, 0 mutations published"));
    }
}
