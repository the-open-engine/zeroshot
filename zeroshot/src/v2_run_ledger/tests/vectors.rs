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

async fn exercise_long_record(ledger: &dyn RunLedger) {
    let source = RunId::new("long-source");
    let successor = RunId::new("long-successor");
    for (run, digest) in [(&source, 'c'), (&successor, 'd')] {
        ledger
            .create_or_get(create(run.as_str(), run.as_str(), digest))
            .await
            .assert_value();
    }
    let mut expected = Vec::new();
    let mut events = Vec::new();
    for index in 0..305 {
        let RunEvent::Vector { mut vector } =
            super::super::vector_event(&source, RunVectorContent::SuspendRequest {}).assert_value()
        else {
            unreachable!()
        };
        // IDs are opaque and need not sort in event order, particularly after inheritance.
        vector.id = openengine_cluster_protocol::VectorId::new(format!("entry-{}", 305 - index))
            .assert_value();
        expected.push(vector.clone());
        events.push(RunEvent::Vector { vector });
        events.push(RunEvent::SafeLog {
            execution: None,
            timestamp: UnixTimestampMillis::new(1).assert_value(),
            stream: SafeLogStream::Output,
            line: SafeLogLine::new("between entries").assert_value(),
            diagnostic: None,
        });
    }
    ledger.append(&source, events).await.assert_value();
    super::super::vectors::inherit(ledger, ledger, &source, &successor)
        .await
        .assert_value();
    let mut inherited = Vec::new();
    let mut after = None;
    loop {
        let page = super::super::vectors::list(
            ledger,
            RunVectorsParams {
                run_id: successor.clone(),
                after,
                limit: Some(100),
            },
        )
        .await
        .assert_value();
        inherited.extend(page.vectors);
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(&inherited[..305], expected);
    assert_eq!(inherited.len(), 306);
    assert!(
        matches!(&inherited[305].content, RunVectorContent::Continuation { resumed_from } if resumed_from == &source)
    );
    for (run_id, error) in [
        (source, RunLedgerError::VectorNotFound),
        (RunId::new("missing"), RunLedgerError::RunNotFound),
    ] {
        assert_eq!(
            super::super::vectors::list(
                ledger,
                RunVectorsParams {
                    run_id,
                    after: Some(inherited[305].id.clone()),
                    limit: None,
                }
            )
            .await,
            Err(error)
        );
    }
}

#[tokio::test]
async fn fake_pages_and_inherits_long_interleaved_records() {
    exercise_long_record(&FakeRunLedger::new()).await;
}

#[tokio::test]
async fn sqlite_pages_and_inherits_long_interleaved_records() {
    let ledger = SqliteRunLedger::open_in_memory().assert_value();
    exercise_long_record(&ledger).await;
}

#[tokio::test]
async fn sqlite_read_only_pagination_seeks_past_earlier_events() {
    let root = tempfile::tempdir().assert_value();
    let path = root.path().join("ledger.sqlite3");
    let ledger = SqliteRunLedger::open(&path).assert_value();
    let run = RunId::new("source");
    ledger
        .create_or_get(create(run.as_str(), run.as_str(), 'e'))
        .await
        .assert_value();
    let first =
        super::super::vector_event(&run, RunVectorContent::SuspendRequest {}).assert_value();
    let second =
        super::super::vector_event(&run, RunVectorContent::SuspendRequest {}).assert_value();
    let RunEvent::Vector {
        vector: first_vector,
    } = &first
    else {
        unreachable!()
    };
    let RunEvent::Vector {
        vector: second_vector,
    } = &second
    else {
        unreachable!()
    };
    let first_id = first_vector.id.clone();
    let expected = second_vector.clone();
    ledger
        .append(&run, vec![RunEvent::RunStarted, first, second])
        .await
        .assert_value();
    drop(ledger);
    let connection = rusqlite::Connection::open(&path).assert_value();
    // A page after the first vector must never decode earlier non-vector events.
    connection.execute(
        "UPDATE v2_run_events SET event_json = '{\"kind\":\"unreadable_old_event\"}' WHERE sequence = 1",
        [],
    ).assert_value();
    drop(connection);
    let ledger = SqliteRunLedger::open_read_only(&path).assert_value();
    let page = super::super::vectors::list(
        &ledger,
        RunVectorsParams {
            run_id: run,
            after: Some(first_id),
            limit: None,
        },
    )
    .await
    .assert_value();
    assert_eq!(page.vectors, vec![expected]);
    assert!(page.next_after.is_none());
}
