//! Seek only steering entries in the sole event record, including on read-only connections.
use super::*;

pub(super) const QUERY: &str = "SELECT sequence, event_json FROM v2_run_events
    WHERE run_id = ?1 AND sequence > ?2
      AND json_extract(event_json, '$.kind') = 'vector'
      AND json_extract(event_json, '$.vector.kind') IN ('steer', 'steer_receipt')
    ORDER BY sequence LIMIT ?3";

const LEGACY_QUERY: &str = "SELECT sequence, event_json FROM v2_run_events
    INDEXED BY v2_run_vector_ids
    WHERE run_id = ?1 AND sequence > ?2
      AND json_extract(event_json, '$.kind') = 'vector'
      AND json_extract(event_json, '$.vector.kind') IN ('steer', 'steer_receipt')
    ORDER BY sequence LIMIT ?3";

pub(super) fn query(connection: &Connection) -> Result<Option<&'static str>, RunLedgerError> {
    let (steering, vectors): (bool, bool) = connection
        .query_row(
            "SELECT
                EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'v2_run_steering' AND type = 'index'),
                EXISTS(SELECT 1 FROM sqlite_schema WHERE name = 'v2_run_vector_ids' AND type = 'index')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(sqlite_error)?;
    // Read-only retained ledgers cannot install the newer index. Seek their existing
    // partial vector index explicitly so SQLite never chooses ordinary event history.
    // Ledgers predating vectors have neither index and cannot contain steers.
    Ok(if steering {
        Some(QUERY)
    } else if vectors {
        Some(LEGACY_QUERY)
    } else {
        None
    })
}

pub(super) fn read(
    connection: &Connection,
    run: &RunId,
    after: Option<&Cursor>,
) -> Result<Vec<StoredRunEvent>, RunLedgerError> {
    let current = connection
        .query_row(
            "SELECT cursor FROM v2_runs WHERE run_id = ?1",
            [run.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(sqlite_error)?
        .ok_or(RunLedgerError::RunNotFound)?;
    let after = after.map_or(Ok(0), cursor_sequence)?;
    let after = i64::try_from(after).map_err(|_| RunLedgerError::CursorAhead)?;
    if after > current {
        return Err(RunLedgerError::CursorAhead);
    }
    let Some(query) = query(connection)? else {
        return Ok(Vec::new());
    };
    let mut statement = connection.prepare(query).map_err(sqlite_error)?;
    let mut rows = statement
        .query(params![run.as_str(), after, MAX_REPLAY_EVENTS as i64])
        .map_err(sqlite_error)?;
    let mut result = Vec::new();
    let mut bytes = 0;
    while let Some(row) = rows.next().map_err(sqlite_error)? {
        let json = row.get_ref(1).map_err(sqlite_error)?;
        let json = json.as_str().map_err(|_| RunLedgerError::Corrupt)?;
        if json.len() > MAX_REPLAY_BYTES {
            return Err(RunLedgerError::Corrupt);
        }
        if bytes + json.len() > MAX_REPLAY_BYTES {
            break;
        }
        bytes += json.len();
        result.push(decode_replay_event(row, json)?);
    }
    Ok(result)
}
