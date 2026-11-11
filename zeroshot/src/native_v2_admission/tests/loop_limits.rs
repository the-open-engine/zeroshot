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
        let mut node:GraphNode=serde_json::from_value(serde_json::json!({"kind":"loop","name":"repeat","state":{"kind":"null"},
                "body":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]},"maxIterations":1,"maxIterationsInput":["rounds"],"promotedStatePaths":[]})).unwrap();
        bind_input_limits(&mut node, &serde_json::json!({"rounds":value})).unwrap();
        let GraphNode::Loop(group) = node else {
            panic!("expected loop")
        };
        assert_eq!(group.max_iterations.get(), expected);
    }
}
