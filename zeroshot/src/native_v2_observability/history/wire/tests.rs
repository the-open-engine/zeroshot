use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;

#[test]
fn terminal_synopsis_never_serializes_success_output() {
    let terminal = TerminalResult::Succeeded {
        output: json!({"private": "large output"}),
    };
    let synopsis = RunHistoryTerminalSynopsis::from(&terminal);
    assert_eq!(
        serde_json::to_value(synopsis).assert_value(),
        json!({"status": "succeeded"})
    );
    assert!(
        serde_json::from_value::<RunHistoryTerminalSynopsis>(json!({
            "status": "succeeded",
            "output": {"private": "large output"}
        }))
        .is_err()
    );
}

#[test]
fn run_history_list_rejects_unknown_summary_fields() {
    let valid = json!({
        "runs": [{
            "runId": "018f5e78-7f95-7c22-8d98-3f15af20c991",
            "title": "Queued run",
            "phase": "queued",
            "cursor": null,
            "terminal": null,
            "source": null,
            "createdAt": null,
            "historyAvailable": false
        }],
        "nextCursor": null
    });
    serde_json::from_value::<RunHistoryList>(valid.clone()).assert_value();
    let mut unknown = valid;
    unknown["runs"][0]["output"] = json!("not a summary field");
    assert!(serde_json::from_value::<RunHistoryList>(unknown).is_err());
}

#[test]
fn history_problem_vocabulary_round_trips_including_incomplete() {
    assert!(HISTORY_PROBLEM_CODES.contains(&HistoryProblemCode::HistoryIncomplete));
    for code in HISTORY_PROBLEM_CODES {
        assert_eq!(HistoryProblemCode::parse(code.as_str()), Some(code));
        assert_eq!(
            serde_json::to_value(code).assert_value(),
            json!(code.as_str())
        );
    }
}

#[test]
fn projected_delivery_execution_ids_never_use_javascript_numbers() {
    let execution = 9_007_199_254_740_993_u64;
    let mut event = json!({"kind":"primary_graph_completed","delivery_execution":execution});
    super::super::project_event(&mut event);
    assert_eq!(event["delivery_execution"], json!(execution.to_string()));
}

#[test]
fn projected_checkpoint_execution_ids_preserve_opaque_precision() {
    let execution = 9_007_199_254_740_993_u64;
    let node_instance = u64::MAX;
    let mut event = json!({
        "kind":"prior_execution",
        "execution": {
            "execution":execution,
            "node_instance":node_instance,
            "occurrence":{"node":"code","mapIndices":[]},
            "state":{"Settled":{"outcome":{"status":"verifier","output":{"value":7}}}}
        }
    });
    super::super::project_event(&mut event);
    assert_eq!(
        event["execution"]["execution"],
        json!(execution.to_string())
    );
    assert_eq!(
        event["execution"]["node_instance"],
        json!(node_instance.to_string())
    );
    assert_eq!(
        event["execution"]["state"]["Settled"]["outcome"]["output"]["value"],
        json!(7)
    );
}
