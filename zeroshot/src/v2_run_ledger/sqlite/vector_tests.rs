use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

#[test]
fn vector_cursor_lookup_uses_the_per_run_index() {
    let connection = Connection::open_in_memory().assert_value();
    connection.execute_batch(SCHEMA).assert_value();
    let query = format!("EXPLAIN QUERY PLAN {VECTOR_CURSOR_QUERY}");
    let mut statement = connection.prepare(&query).assert_value();
    let plan = statement
        .query_map(["run", "entry"], |row| row.get::<_, String>(3))
        .assert_value()
        .map(|row| row.assert_value())
        .collect::<Vec<_>>();
    assert!(
        plan.iter().any(
            |line| line.contains("SEARCH v2_run_events USING INDEX v2_run_vector_ids")
                && line.contains("<expr>=?")
        ),
        "{plan:?}"
    );
}
