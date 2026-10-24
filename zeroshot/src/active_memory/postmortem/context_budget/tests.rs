use super::*;
#[test]
fn quote_heavy_history_and_supplied_cards_fit_the_serialized_role_budget() {
    let quoted = "\\\"".repeat(2048);
    let mut document = base(json!({"evidence":{"records":vec![json!(quoted);128],
        "executions":vec![json!(quoted);128],"authored_graph":quoted,"initial_input":quoted},
        "supplied_lessons":{},"existing_lessons":[]}));
    let mut inserted = 0;
    for index in 0..128 {
        if !add_object(
            &mut document,
            "supplied_lessons",
            &format!("lesson-{index}"),
            json!({"recommendation":quoted}),
        ) {
            break;
        }
        inserted += 1;
    }
    assert!(inserted > 0 && inserted < 128);
    assert_eq!(document["evidence"]["evidence_truncated"], true);
    assert!(encoded_size(&document) <= MAX_INPUT_BYTES);
}

#[test]
fn oversized_cards_are_rejected_whole_and_leave_existing_context_unchanged() {
    let mut document = json!({"existing_lessons":[], "supplied_lessons":{}});
    assert!(add_array(
        &mut document,
        "existing_lessons",
        json!({"id":"first"})
    ));
    assert!(add_object(
        &mut document,
        "supplied_lessons",
        "first",
        json!({"id":"first"})
    ));
    let original = document.clone();
    let large = json!({"recommendation":"\"".repeat(MAX_INPUT_BYTES)});
    assert!(!add_array(&mut document, "existing_lessons", large.clone()));
    assert_eq!(document, original);
    assert!(!add_object(
        &mut document,
        "supplied_lessons",
        "too-large",
        large
    ));
    assert_eq!(document, original);
    assert!(!add_array(&mut document, "missing", json!(null)));
    assert!(!add_object(&mut document, "missing", "id", json!(null)));
}

#[test]
fn evidence_truncation_retains_early_and_recent_tool_receipts() {
    let records: Vec<_> = (0..64)
        .map(|seq| json!({"seq":seq,"paths":["a".repeat(2048)]}))
        .collect();
    let document =
        base(json!({"evidence":{"records":[],"executions":[],"tool_observations":records}}));
    let retained = document["evidence"]["tool_observations"]
        .as_array()
        .unwrap();
    assert!(retained.len() < 64 && !retained.is_empty());
    assert_eq!(retained.first().unwrap()["seq"], 0);
    assert_eq!(retained.last().unwrap()["seq"], 63);
    assert_eq!(document["evidence"]["evidence_truncated"], true);
    assert!(encoded_size(&document) <= BASE_BYTES);
    let one_large_receipt = base(
        json!({"evidence":{"records":[],"executions":[],"tool_observations":[{"paths":["a".repeat(MAX_INPUT_BYTES)]}]}}),
    );
    assert!(
        one_large_receipt["evidence"]["tool_observations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn oversized_graph_text_and_structured_input_have_explicit_bounded_fallbacks() {
    let large = "quoted \" graph".repeat(MAX_INPUT_BYTES / 8);
    let graph = base(json!({"evidence":{"records":[],"executions":[],"authored_graph":large}}));
    assert!(graph["evidence"]["authored_graph"].as_str().unwrap().len() < large.len());
    assert!(encoded_size(&graph) <= BASE_BYTES);
    let input = base(
        json!({"evidence":{"records":[],"executions":[],"initial_input":{"payload":large}}, "supplied_lessons":{}}),
    );
    assert_eq!(input["evidence"]["evidence_truncated"], true);
    assert!(input["evidence"]["omitted_reason"].is_string());
    assert_eq!(input["supplied_lessons"], json!({}));
    assert!(encoded_size(&input) <= BASE_BYTES);
}

#[test]
fn receipt_heavy_context_preserves_a_quiet_workers_transcript() {
    let mut records =
        vec![json!({"execution":1,"reference":"v2:1","event":"worker established the cause"})];
    records.extend((2..52).map(|sequence| json!({"execution":2,"reference":format!("v2:{sequence}"),"event":"reviewer text".repeat(40)})));
    let tools = (1..=512)
        .map(|seq| json!({"seq":seq,"node_id":"worker","paths":vec!["a".repeat(256);4]}))
        .collect::<Vec<_>>();
    let document =
        base(json!({"evidence":{"records":records,"executions":[],"tool_observations":tools}}));
    assert_eq!(document["evidence"]["evidence_truncated"], true);
    assert!(encoded_size(&document) <= BASE_BYTES);
    assert!(
        document["evidence"]["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["reference"] == "v2:1")
    );
}

#[test]
fn escaped_transcripts_are_trimmed_within_each_execution_without_losing_quiet_workers() {
    let event = crate::native_v2_runner::bounded_log_text(
        json!({"kind":"safe_log","execution":2,"line":"\\\"".repeat(256)}).to_string(),
        1024,
    );
    let mut records =
        vec![json!({"execution":1,"reference":"v2:1","event":"worker experiment found the cause"})];
    records.extend(
        (2..32).map(
            |sequence| json!({"execution":2,"reference":format!("v2:{sequence}"),"event":event}),
        ),
    );
    let input = json!({"evidence":{"records":records,"executions":[],"tool_observations":[],"authored_graph":"node instructions ".repeat(800)}});
    assert!(encoded_size(&input) > BASE_BYTES);
    let document = base(input);
    let retained = document["evidence"]["records"].as_array().unwrap();
    assert!(retained.len() < 31);
    assert!(retained.iter().any(|record| record["reference"] == "v2:1"));
    assert!(retained.iter().any(|record| record["reference"] == "v2:31"));
    assert_eq!(document["evidence"]["evidence_truncated"], true);
    assert!(encoded_size(&document) <= BASE_BYTES);
}

#[test]
fn unshrinkable_evidence_preserves_the_complete_nonempty_delivery_manifest() {
    let supplied = json!({"card-1":{"id":"card-1","revision":7}});
    let records = vec![
        json!({"execution":1,"reference":"v2:1","event":"worker evidence"}),
        json!({"execution":2,"reference":"v2:2","event":"reviewer evidence"}),
    ];
    let document = base(
        json!({"manifest_known":true,"supplied_lessons":supplied,"evidence":{"records":records,"impact_episodes":{"oversized_metadata":"x".repeat(MAX_INPUT_BYTES)}}}),
    );
    assert_eq!(document["manifest_known"], true);
    assert_eq!(document["supplied_lessons"], supplied);
    assert_eq!(document["evidence"]["evidence_truncated"], true);
    assert!(document["evidence"]["omitted_reason"].is_string());
    assert!(encoded_size(&document) <= BASE_BYTES);
}
