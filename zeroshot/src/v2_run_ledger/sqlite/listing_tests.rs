use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

fn id(index: usize) -> RunId {
    RunId::new(format!("01960000-0000-7000-8000-{index:012x}"))
}

#[tokio::test]
async fn bounded_id_pages_ignore_payloads_and_exclude_newer_insertions() {
    let connection = Connection::open_in_memory().assert_value();
    connection.execute_batch(SCHEMA).assert_value();
    for index in [3, 1, 5, 2, 4] {
        connection
            .execute(
                "INSERT INTO v2_runs VALUES (?1, ?1, 'digest', 0, 'invalid run payload')",
                params![id(index).as_str()],
            )
            .assert_value();
    }
    let ledger = SqliteRunLedger::from_connection(connection).assert_value();
    let first = ledger.list_ids_page(None, 2).await.assert_value();
    assert_eq!(first, vec![id(5), id(4)]);

    ledger
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO v2_runs VALUES (?1, ?1, 'digest', 0, 'invalid run payload')",
                    params![id(6).as_str()],
                )
                .map_err(sqlite_error)?;
            Ok(())
        })
        .await
        .assert_value();
    assert_eq!(
        ledger.list_ids_page(first.last(), 2).await.assert_value(),
        vec![id(3), id(2)]
    );
    assert_eq!(
        ledger.list_ids_page(Some(&id(2)), 2).await.assert_value(),
        vec![id(1)]
    );
    assert!(
        ledger
            .list_ids_page(Some(&id(1)), 2)
            .await
            .assert_value()
            .is_empty()
    );
    assert!(
        ledger
            .list_ids_page(None, 0)
            .await
            .assert_value()
            .is_empty()
    );
}
