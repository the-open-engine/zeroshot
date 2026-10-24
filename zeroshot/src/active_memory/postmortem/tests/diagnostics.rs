use super::*;

#[tokio::test]
async fn rejected_discovery_reports_the_gate_without_publishing_proposal_text() {
    for (field, replacement, diagnostic) in [
        ("node_id", json!("invented_node"), "unknown executed node"),
        (
            "references",
            json!(["hook:1", "hook:2", "repository:src/provider.rs"]),
            "required main-run cost",
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
