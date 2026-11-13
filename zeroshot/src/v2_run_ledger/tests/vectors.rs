use super::*;
use openengine_cluster_protocol::{RunVectorContent, RunVectorsParams, RunVectorParams};

async fn exercise(ledger: &dyn RunLedger) {
    let source = RunId::new("source");
    let successor = RunId::new("successor");
    ledger
        .create_or_get(create(source.as_str(), "source", 'a'))
        .await
        .assert_value();
    ledger
        .append(&source, vec![RunEvent::RunStarted])
        .await
        .assert_value();
    ledger.request_suspend(&source).await.assert_value();
    let before = ledger
        .get(&source)
        .await
        .assert_value()
        .assert_value()
        .snapshot
        .cursor;
    ledger.request_suspend(&source).await.assert_value();
    assert_eq!(
        ledger
            .get(&source)
            .await
            .assert_value()
            .assert_value()
            .snapshot
            .cursor,
        before
    );
    let suspension = openengine_cluster_protocol::RunSuspension::Command {
        node: NodeName::new("next").assert_value(),
        created_at: UnixTimestampMillis::new(1_762_905_600_000).assert_value(),
    };
    ledger
        .append(
            &source,
            vec![
                super::super::vector_event(
                    &source,
                    RunVectorContent::Suspension {
                        reason: openengine_cluster_protocol::VectorSuspensionReason::Command,
                        node: suspension.node().clone(),
                    },
                )
                .assert_value(),
                RunEvent::Suspended { suspension },
            ],
        )
        .await
        .assert_value();
    assert!(ledger.request_suspend(&source).await.is_err());
    ledger
        .create_or_get(create(successor.as_str(), "successor", 'b'))
        .await
        .assert_value();
    super::super::vectors::inherit(ledger, ledger, &source, &successor)
        .await
        .assert_value();
    let original = super::super::vectors::list(
        ledger,
        RunVectorsParams {
            run_id: source.clone(),
            after: None,
            limit: None,
        },
    )
    .await
    .assert_value();
    let page = super::super::vectors::list(
        ledger,
        RunVectorsParams {
            run_id: successor.clone(),
            after: None,
            limit: Some(2),
        },
    )
    .await
    .assert_value();
    assert_eq!(page.vectors, original.vectors);
    let tail = super::super::vectors::list(
        ledger,
        RunVectorsParams {
            run_id: successor.clone(),
            after: page.next_after,
            limit: Some(2),
        },
    )
    .await
    .assert_value();
    assert_eq!(tail.vectors.len(), 1);
    assert!(
        matches!(&tail.vectors[0].content,RunVectorContent::Continuation{resumed_from} if resumed_from==&source)
    );
    assert!(tail.next_after.is_none());
    let entry = page.vectors[0].clone();
    assert_eq!(
        super::super::vectors::show(
            ledger,
            RunVectorParams {
                run_id: successor,
                id: entry.id.clone()
            }
        )
        .await
        .assert_value(),
        entry
    );
}

#[tokio::test]
async fn fake_record_retains_order_and_identity_across_successors() {
    exercise(&FakeRunLedger::new()).await;
}
#[tokio::test]
async fn sqlite_record_retains_order_and_identity_across_successors() {
    let root = tempfile::tempdir().assert_value();
    let ledger = SqliteRunLedger::open(root.path().join("ledger.sqlite3")).assert_value();
    exercise(&ledger).await;
}
