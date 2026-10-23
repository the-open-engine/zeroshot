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
