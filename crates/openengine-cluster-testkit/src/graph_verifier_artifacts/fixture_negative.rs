pub(super) fn duplicate_node_graph() -> Value {
    graph(
        null_type(),
        null_type(),
        vec![null_step("work", "fixture.worker@1"), succeed("work")],
    )
}

pub(super) fn fallthrough_graph() -> Value {
    json!({
        "profile":"openengine.graph.full/v1","initialInput":null_type(),
        "policy":{"policy":"policy.strict@1","default":"deny"},
        "root":null_step("work", "fixture.worker@1")
    })
}

pub(super) fn illegal_control_graph() -> Value {
    graph(
        null_type(),
        null_type(),
        vec![
            null_step("work", "fixture.worker@1"),
            choice(
                "decision",
                null_type(),
                json!({
                    "kind":"in","value":{"name":"work","source":"signal","field":"verdict"},
                    "labels":["accepted"]
                }),
                succeed("selected"),
            ),
        ],
    )
}

pub(super) fn undefined_read_graph() -> Value {
    let mut step = data_step("work");
    step["inputBindings"][0]["value"]["path"] = json!(["result"]);
    graph(
        data_state_type(),
        data_state_type(),
        vec![step, succeed("done")],
    )
}

pub(super) fn output_write_error_path_graph() -> Value {
    graph(
        data_state_type(),
        data_state_type(),
        vec![data_step("produce"), state_result_succeed("done")],
    )
}

pub(super) fn cyclic_read_graph() -> Value {
    let mut left = data_step("left");
    left["writeBindings"][0]["value"]["node"] = json!("right");
    let mut right = data_step("right");
    right["writeBindings"][0]["value"]["node"] = json!("left");
    graph(
        data_state_type(),
        data_state_type(),
        vec![left, right, succeed("done")],
    )
}

pub(super) fn type_mismatch_graph() -> Value {
    let mut step = data_step("work");
    step["input"]["fields"]["value"]["type"] = json!({"kind":"string"});
    graph(
        data_state_type(),
        data_state_type(),
        vec![step, succeed("done")],
    )
}

pub(super) fn dead_choice_graph() -> Value {
    let decision = json!({
        "kind":"choice","name":"decision","state":data_state_type(),
        "branches":[
            {"when":in_guard(),"node":succeed("first")},
            {"when":in_guard(),"node":null_step("dead", "fixture.worker@1")}
        ],
        "otherwise":fail("otherwise"),"promotedStatePaths":[]
    });
    graph(
        data_state_type(),
        data_state_type(),
        vec![verifier_node("verify"), decision],
    )
}

pub(super) fn dead_otherwise_graph() -> Value {
    let decision = json!({
        "kind":"choice","name":"decision","state":data_state_type(),
        "branches":[
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["accepted"]},
                "node":succeed("accepted")
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["rejected"]},
                "node":fail("rejected")
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":fail("workerFailed")
            }
        ],
        "otherwise":null_step("deadOtherwise", "fixture.worker@1"),"promotedStatePaths":[]
    });
    graph(
        data_state_type(),
        data_state_type(),
        vec![verifier_node("verify"), decision],
    )
}

pub(super) fn non_exhaustive_choice_graph() -> Value {
    let mut decision = choice(
        "decision",
        data_state_type(),
        in_guard(),
        succeed("selected"),
    );
    decision["otherwise"] = Value::Null;
    graph(
        data_state_type(),
        data_state_type(),
        vec![verifier_node("verify"), decision],
    )
}

pub(super) fn unsatisfiable_loop_graph() -> Value {
    let contradictory = json!({"kind":"all","guards":[
        in_guard(),{"kind":"not","guard":in_guard()}
    ]});
    let loop_node = json!({
        "kind":"loop","name":"repeat","state":data_state_type(),"body":verifier_node("verify"),
        "until":contradictory,"maxIterations":2,"promotedStatePaths":[]
    });
    graph(
        data_state_type(),
        data_state_type(),
        vec![loop_node, succeed("done")],
    )
}

pub(super) fn invalid_quorum_graph() -> Value {
    let par = json!({
        "kind":"par","name":"parallel","state":null_type(),
        "branches":[null_step("left", "fixture.worker@1"),null_step("right", "fixture.worker@1")],
        "promotedStatePaths":[],"join":{"kind":"quorum","count":3}
    });
    graph(null_type(), null_type(), vec![par, succeed("done")])
}

pub(super) fn write_conflict_graph() -> Value {
    let par = json!({
        "kind":"par","name":"parallel","state":data_state_type(),
        "branches":[data_step("left"),data_step("right")],
        "promotedStatePaths":[],"join":{"kind":"all"}
    });
    graph(
        data_state_type(),
        data_state_type(),
        vec![par, succeed("done")],
    )
}

pub(super) fn unsafe_promotion_graph() -> Value {
    let decision = json!({
        "kind":"choice","name":"promote","state":data_state_type(),
        "branches":[{"when":in_guard(),"node":data_step("work")}],
        "otherwise":fail("failed"),"promotedStatePaths":[["result"]]
    });
    graph(
        data_state_type(),
        data_state_type(),
        vec![verifier_node("verify"), decision, succeed("done")],
    )
}

pub(super) fn impossible_map_outcomes_graph() -> Value {
    let map = json!({
        "kind":"map","name":"map","state":data_state_type(),"body":verifier_node("mapVerify"),
        "over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[]
    });
    let guard = json!({"kind":"all","guards":[
        {"kind":"k_of_map","count":1,"value":{"name":"mapVerify","source":"signal","field":"verdict"},"labels":["accepted"]},
        {"kind":"k_of_map","count":1,"value":{"name":"mapVerify","source":"error","field":null},"labels":["timeout"]}
    ]});
    graph(
        data_state_type(),
        data_state_type(),
        vec![
            map,
            choice("decision", data_state_type(), guard, succeed("selected")),
        ],
    )
}

pub(super) fn closed_k_labels_graph() -> Value {
    let guard = json!({
        "kind":"k_of_n","count":1,
        "values":[
            {"name":"verify","source":"signal","field":"verdict"},
            {"name":"verify","source":"error","field":null}
        ],
        "labels":["accepted","timeout","bogus"]
    });
    guarded_graph(guard)
}

pub(super) fn worker_graph(worker: &str) -> Value {
    graph(
        null_type(),
        null_type(),
        vec![null_step("work", worker), succeed("done")],
    )
}

pub(super) fn registry_input_graph() -> Value {
    let mut step = null_step("work", "fixture.worker@1");
    step["input"] = json!({"kind":"record","fields":{}});
    graph(null_type(), null_type(), vec![step, succeed("done")])
}

pub(super) fn registry_output_graph() -> Value {
    let step = json!({
        "kind":"step","name":"work","worker":"fixture.data@1",
        "input":data_input_type(),"output":null_type(),
        "inputBindings":[{"target":["value"],"value":{"source":"state","path":["value"]}}],
        "writeBindings":[],"timeoutMs":1,"attempts":1
    });
    graph(
        data_state_type(),
        data_state_type(),
        vec![step, succeed("done")],
    )
}

pub(super) fn registry_verifier_contract_graph() -> Value {
    let step = json!({
        "kind":"step","name":"work","worker":"fixture.verifier@1",
        "input":null_type(),"output":result_number_type(),
        "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1
    });
    graph(null_type(), null_type(), vec![step, succeed("done")])
}

pub(super) fn registry_signal_field_graph() -> Value {
    let mut verifier = verifier_node("verify");
    verifier["signals"] = json!({"other":["accepted"]});
    graph(null_type(), null_type(), vec![verifier, succeed("done")])
}

pub(super) fn registry_signal_labels_graph() -> Value {
    let mut verifier = verifier_node("verify");
    verifier["signals"] = json!({"verdict":["accepted"]});
    graph(null_type(), null_type(), vec![verifier, succeed("done")])
}

pub(super) fn registry_diagnostic_graph() -> Value {
    let mut verifier = verifier_node("verify");
    verifier["diagnostic"] = json!({"kind":"string"});
    graph(null_type(), null_type(), vec![verifier, succeed("done")])
}

pub(super) fn null_type() -> Value {
    json!({"kind":"null"})
}

pub(super) fn data_input_type() -> Value {
    json!({"kind":"record","fields":{"value":{"type":{"kind":"integer"},"required":true}}})
}

pub(super) fn result_integer_type() -> Value {
    json!({"kind":"record","fields":{"result":{"type":{"kind":"integer"},"required":true}}})
}

pub(super) fn result_number_type() -> Value {
    json!({"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}})
}

pub(super) fn diagnostic_integer_type() -> Value {
    json!({"kind":"record","fields":{"code":{"type":{"kind":"integer"},"required":true}}})
}

pub(super) fn diagnostic_number_type() -> Value {
    json!({"kind":"record","fields":{"code":{"type":{"kind":"number"},"required":true}}})
}

pub(super) fn data_state_type() -> Value {
    json!({
        "kind":"record",
        "fields":{
            "value":{"type":{"kind":"integer"},"required":true},
            "items":{"type":{"kind":"array","items":data_input_type()},"required":true},
            "result":{"type":{"kind":"number"},"required":false},
            "verdict":{"type":{"kind":"enum","values":["accepted","rejected"]},"required":false},
            "diagnostic":{"type":{"kind":"number"},"required":false}
        }
    })
}

pub(super) fn worker_ref(value: &str) -> WorkerRef {
    WorkerRef::new(value).expect("fixture worker reference is valid")
}

pub(super) fn json_artifact(relative_path: String, value: Value) -> Artifact {
    let mut bytes = serde_json::to_vec_pretty(&value).expect("fixture serialization must succeed");
    bytes.push(b'\n');
    Artifact {
        relative_path,
        bytes,
    }
}
use super::fixture_builders::*;
use super::fixture_controls::guarded_graph;
use super::*;
