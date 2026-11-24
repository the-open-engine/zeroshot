use super::*;
use std::{collections::HashMap, convert::Infallible, future::ready};

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
