use super::*;
use std::{collections::HashMap, convert::Infallible, future::ready};

#[tokio::test]
async fn cleanup_reads_each_confirmed_family_once_in_any_inventory_order() {
    let [root, middle, head] = ["root", "middle", "head"].map(RunId::new);
    let calls = std::cell::Cell::new(0);
    let read = |id: RunId| {
        calls.set(calls.get() + 1);
        let links = if id == root {
            (None, Some(middle.clone()))
        } else if id == middle {
            (Some(root.clone()), Some(head.clone()))
        } else {
            (Some(middle.clone()), None)
        };
        ready(Ok::<_, Infallible>(Some(WorkspaceRecovery {
            resumed_from: links.0,
            successor_run_id: links.1,
            ..Default::default()
        })))
    };
    let mut superseded = SupersededParts::default();
    for id in [&middle, &head, &root] {
        assert_eq!(
            superseded
                .contains(id, RunLineage::resolve(id, read))
                .await
                .unwrap(),
            id != &head
        );
    }
    assert_eq!(calls.get(), 3);
}

#[test]
fn grouped_history_preserves_worker_owned_definition_documents() {
    use crate::native_v2_observability::history::parts::grouped_definition;
    use serde_json::json;

    let parts = [RunId::new("original"), RunId::new("resumed")];
    let graph = json!({"version": 1, "nodes": []});
    let runtime = json!({"workerSettings": {"provider": "archived"}});
    let saved = json!({
        "runId": "resumed", "phase": "suspended", "cursor": "v2:4",
        "history": {"complete": true}, "graph": graph, "runtime": runtime,
    });
    let grouped = grouped_definition(&parts, saved).expect("validated history envelope");
    assert_eq!(grouped["graph"], graph);
    assert_eq!(grouped["runtime"], runtime);
    assert_eq!(grouped["runId"], "original");
    assert_eq!(grouped["history"]["cursor"], "resumed@v2:4");
    assert_eq!(grouped["history"]["complete"], false);
}

#[test]
fn archived_failure_links_stay_with_the_part_that_recorded_them() {
    use crate::native_v2_observability::history::parts::grouped_definition;
    use serde_json::json;

    let parts = ["original", "second"].map(RunId::new);
    let context = json!({
        "primary": {"cursor": "v2:2", "message": "failed"},
        "recorded": [{"cursor": "v2:1", "message": "first error"}],
        "verifiers": [{"message": "no ledger position"}],
    });
    let archived = json!({
        "runId": "second", "phase": "finished", "cursor": "v2:4", "history": {"complete": true},
        "runtimeFailure": {"atCursor": "v2:3", "context": context},
        "terminal": {"status": "failed", "context": context},
    });
    assert!(grouped_definition(&parts[..1], archived.clone()).is_none());
    let grouped = grouped_definition(&parts, archived).unwrap();
    assert_eq!(grouped["history"]["complete"], true);
    assert_eq!(grouped["runtimeFailure"]["runCursor"], "second@v2:3");
    for ctx in [
        &grouped["runtimeFailure"]["context"],
        &grouped["terminal"]["context"],
    ] {
        assert_eq!(ctx["primary"]["cursor"], "v2:2");
        assert_eq!(ctx["primary"]["runCursor"], "second@v2:2");
        assert_eq!(ctx["recorded"][0]["runCursor"], "second@v2:1");
        assert!(ctx["verifiers"][0].get("runCursor").is_none());
    }
}

#[test]
fn replay_separates_imported_graph_state_from_work_the_successor_performed() {
    use crate::native_v2_observability::history::{parts::RunPartsPage, HistoryPage};
    use serde_json::json;

    let parts = ["original", "retry"].map(RunId::new);
    let control = |cursor| {
        json!({
            "cursor": cursor, "node": "approval", "mapIndices": [], "visitId": "visit", "state": "reached",
        })
    };
    let physical: HistoryPage = serde_json::from_value(json!({
        "events": [
            {"cursor": "v2:1", "event": {"kind": "prior_execution"}},
            {"cursor": "v2:2", "event": {"kind": "vector", "vector": {"runId": "original"}}},
            {"cursor": "v2:3", "event": {"kind": "vector", "vector": {"runId": "retry"}}},
            {"cursor": "v2:4", "event": {"kind": "terminal", "result": {"context": {
                "primary": {"cursor": "v2:3", "message": "failure"}
            }}}},
        ],
        "control": [control("v2:1"), control("v2:3")],
        "nextCursor": "v2:4", "headCursor": "v2:4", "complete": true,
        "finished": true, "observation": {"state": "complete"},
    }))
    .unwrap();
    let view = RunPartsPage::from_part(&parts, (1, Cursor::new("v2:0")), physical, None)
        .unwrap()
        .value();
    assert_eq!(view["imports"][0]["runCursor"], "retry@v2:1");
    assert_eq!(view["importedControl"][0]["runCursor"], "retry@v2:1");
    assert_eq!(view["omittedCursors"], json!(["v2:2"]));
    assert_eq!(view["control"].as_array().unwrap().len(), 1);
    assert_eq!(view["control"][0]["runCursor"], "retry@v2:3");
    let events = view["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["cursor"], "v2:3");
    assert_eq!(events[0]["runCursor"], "retry@v2:3");
    assert_eq!(
        events[1]["event"]["result"]["context"]["primary"]["runCursor"],
        "retry@v2:3"
    );
    assert_eq!(view["partNextCursor"], "v2:4");
    assert_eq!(view["nextCursor"], "retry@v2:4");
}

#[test]
fn provisional_failure_diagnostics_link_to_logical_logs_once() {
    let mut status: RunStatus = serde_json::from_value(serde_json::json!({
        "phase":"running", "activeExecutions":[], "graphResult":{
            "status":"failed", "reason":"runtime_failed", "context":{
                "primary":{"message":"failure","cursor":"v2:3"},
                "recorded":[{"message":"note"},{"message":"first failure","cursor":"v2:2"}],
                "verifiers":[{"message":"verification","cursor":"v2:1"}]
            }
        }
    }))
    .unwrap();
    let part = RunId::new("second");
    scope_diagnostic_cursors(&part, &mut status);
    scope_diagnostic_cursors(&part, &mut status);
    let value = serde_json::to_value(status).unwrap();
    let context = &value["graphResult"]["context"];
    assert_eq!(context["primary"]["cursor"], "second@v2:3");
    assert!(context["recorded"][0].get("cursor").is_none());
    assert_eq!(context["recorded"][1]["cursor"], "second@v2:2");
    assert_eq!(context["verifiers"][0]["cursor"], "second@v2:1");
}

#[tokio::test]
async fn only_reciprocal_handoffs_join_parts_and_scoped_cursors_stay_with_their_ledger() {
    let [root, middle, tip, pending] = ["root", "earlier", "current", "unclaimed"].map(RunId::new);
    let records: HashMap<_, _> = [
        (&root, None, Some(&middle)),
        (&middle, Some(&root), Some(&tip)),
        (&tip, Some(&middle), Some(&pending)),
        (&pending, Some(&middle), None),
    ]
    .into_iter()
    .map(|(id, parent, successor)| {
        (
            id.clone(),
            WorkspaceRecovery {
                resumed_from: parent.cloned(),
                successor_run_id: successor.cloned(),
                ..Default::default()
            },
        )
    })
    .collect();
    let read = |id: RunId| ready(Ok::<_, Infallible>(records.get(&id).cloned()));
    for alias in [&root, &middle, &tip] {
        let family = RunLineage::resolve(alias, read).await.unwrap();
        assert_eq!(family.parts(), &[root.clone(), middle.clone(), tip.clone()]);
        let raw = Cursor::new("v2:4");
        let scoped = part_cursor(&middle, &raw);
        assert_eq!(
            family
                .stream_position(alias, StreamSelection::Execution, Some(scoped.clone()))
                .unwrap(),
            (middle.clone(), Some(raw))
        );
        assert!(
            family
                .stream_position(alias, StreamSelection::Part(&tip), Some(scoped))
                .is_err()
        );
        assert!(
            family
                .stream_position(alias, StreamSelection::Part(&pending), None)
                .is_err()
        );
    }
    assert_eq!(
        RunLineage::resolve(&pending, read).await.unwrap().parts(),
        &[pending]
    );
}

#[tokio::test]
async fn old_bookmarks_resume_the_addressed_part_and_fresh_streams_choose_their_own_start() {
    let [root, tip] = ["root", "part@two"].map(RunId::new);
    let read = |id: RunId| {
        ready(Ok::<_, Infallible>(Some(WorkspaceRecovery {
            resumed_from: (id == tip).then(|| root.clone()),
            successor_run_id: (id == root).then(|| tip.clone()),
            ..Default::default()
        })))
    };
    let run = RunLineage::resolve(&tip, read).await.unwrap();
    let cursor = Cursor::new("v2:5");
    let scoped = part_cursor(&tip, &cursor);
    assert_eq!(split_cursor(&scoped), Some((tip.clone(), cursor.clone())));
    for (address, selection, supplied, expected, offset) in [
        (&tip, StreamSelection::All, None, &root, None),
        (&root, StreamSelection::Execution, None, &tip, None),
        (
            &tip,
            StreamSelection::All,
            Some(cursor.clone()),
            &tip,
            Some(cursor.clone()),
        ),
        (
            &root,
            StreamSelection::Part(&tip),
            Some(cursor.clone()),
            &tip,
            Some(cursor.clone()),
        ),
        (
            &root,
            StreamSelection::All,
            Some(scoped),
            &tip,
            Some(cursor),
        ),
    ] {
        assert_eq!(
            run.stream_position(address, selection, supplied).unwrap(),
            (expected.clone(), offset)
        );
    }
    assert!(
        run.stream_position(&root, StreamSelection::All, Some(Cursor::new("other@v2:5")))
            .is_err()
    );
}

#[tokio::test]
async fn corrupt_or_unreadable_links_cannot_be_reported_as_a_complete_run() {
    let [a, b] = ["a", "b"].map(RunId::new);
    let cycle = |id: RunId| {
        let neighbor = if id == a { &b } else { &a };
        ready(Ok::<_, Infallible>(Some(WorkspaceRecovery {
            resumed_from: Some(neighbor.clone()),
            successor_run_id: Some(neighbor.clone()),
            ..Default::default()
        })))
    };
    assert!(matches!(
        RunLineage::resolve(&a, cycle).await,
        Err(LineageError::Cycle)
    ));
    let unreadable = |id: RunId| {
        ready(if id == a {
            Ok(Some(WorkspaceRecovery {
                successor_run_id: Some(b.clone()),
                ..Default::default()
            }))
        } else {
            Err("storage unavailable")
        })
    };
    assert!(matches!(
        RunLineage::resolve(&a, unreadable).await,
        Err(LineageError::Read("storage unavailable"))
    ));
}
