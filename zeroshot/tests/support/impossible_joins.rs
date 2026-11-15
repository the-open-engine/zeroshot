use serde_json::{Value, json};
use crate::reducer_test_support::verifier;

pub fn impossible_joins() -> impl Iterator<Item = (Value, Vec<Value>)> {
    let failed = json!({"kind":"fail","name":"branch_failed","reason":"branch_failed"});
    let pending = verifier("pending", 1);
    [
        (json!({"kind":"all"}), vec![failed.clone(), pending.clone()]),
        (json!({"kind":"any"}), vec![failed.clone()]),
        (json!({"kind":"quorum","count":2}), vec![failed, pending]),
    ]
    .into_iter()
}
