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
