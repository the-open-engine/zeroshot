use super::super::loop_limits::bind_input_limits;
use openengine_cluster_protocol::GraphNode;
use serde_json::Value;
#[test]
fn input_limits_use_the_existing_positive_safe_integer_wire_rule() {
    for representation in ["3", "3.0", "1000", "1e3"] {
        let value: Value = serde_json::from_str(representation).unwrap();
        let expected = if representation.starts_with('3') {
            3
        } else {
            1000
        };
        let mut node = parameterized_loop();
        bind_input_limits(&mut node, &serde_json::json!({"rounds":value})).unwrap();
        let GraphNode::Loop(group) = node else {
            panic!("expected loop")
        };
        assert_eq!(group.max_iterations.get(), expected);
    }
}

#[test]
fn input_limits_reject_nonpositive_fractional_and_unsafe_values() {
    for value in [
        serde_json::json!(0),
        serde_json::json!(-1),
        serde_json::json!(0.5),
        serde_json::json!("3"),
        Value::Null,
        serde_json::json!(9007199254740992_u64),
    ] {
        let mut node = parameterized_loop();
        assert!(matches!(
            bind_input_limits(&mut node, &serde_json::json!({"rounds":value})),
            Err(super::NativeV2AdmissionError::InvalidLoopLimit { .. })
        ));
    }
}

fn parameterized_loop() -> GraphNode {
    serde_json::from_value(serde_json::json!({"kind":"loop","name":"repeat","state":{"kind":"null"},
        "body":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]},"maxIterations":1,"maxIterationsInput":["rounds"],"promotedStatePaths":[]})).unwrap()
}
