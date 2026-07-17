fn map_state(results_type: Value) -> Value {
    json!({
        "kind":"record",
        "fields":{
            "value":{"type":{"kind":"integer"},"required":true},
            "items":{
                "type":{
                    "kind":"array",
                    "items":{
                        "kind":"record",
                        "fields":{
                            "value":{"type":{"kind":"integer"},"required":true}
                        }
                    }
                },
                "required":true
            },
            "results":{
                "type":results_type,
                "required":false
            }
        }
    })
}

fn mapped_step(result_type: Value, write_bindings: Value) -> Value {
    json!({
        "kind":"step","name":"mapped","worker":"worker.main@1",
        "input":record(),
        "output":{
            "kind":"record",
            "fields":{"result":{"type":result_type,"required":true}}
        },
        "inputBindings":[{
            "target":["value"],
            "value":{"source":"item","path":["value"]}
        }],
        "writeBindings":write_bindings,
        "timeoutMs":1,"attempts":1
    })
}

fn map_result_route(state: &Value) -> Value {
    json!({
        "kind":"choice","name":"afterMap","state":state.clone(),
        "branches":[{
            "when":{
                "kind":"k_of_map",
                "count":1,
                "value":{"name":"mapped","source":"error","field":null},
                "labels":["timeout","crash","malformed","refusal"]
            },
            "node":{
                "kind":"succeed","name":"mappedFailureHandled",
                "output":{"kind":"null"},"bindings":[]
            }
        }],
        "otherwise":{
            "kind":"succeed","name":"completed",
            "output":{
                "kind":"record",
                "fields":{
                    "results":{
                        "type":{"kind":"array","items":{"kind":"integer"}},
                        "required":true
                    }
                }
            },
            "bindings":[{
                "target":["results"],
                "value":{"source":"state","path":["results"]}
            }]
        },
        "promotedStatePaths":[]
    })
}

pub(super) fn indexed_map_graph(
    result_type: Value,
    results_type: Value,
    writes_result: bool,
) -> GraphSpec {
    let state = map_state(results_type);
    let write_bindings = if writes_result {
        json!([{
            "value":{"node":"mapped","channel":"out","path":["result"]},
            "target":["results"]
        }])
    } else {
        json!([])
    };
    graph_with_state_children(
        state.clone(),
        json!([
                {
                    "kind":"map","name":"map","state":state,
                    "body":mapped_step(result_type, write_bindings),
                    "over":{"source":"state","path":["items"]},
                    "maxItems":2,
                    "promotedStatePaths":[["results"]]
                },
                map_result_route(&state)
        ]),
    )
}
use super::*;
