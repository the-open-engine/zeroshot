use serde_json::{Value, json};

pub(super) fn graph() -> Value {
    let worker_errors = json!({
        "kind":"in",
        "value":{"name":"worker","source":"error","field":null},
        "labels":["timeout","crash","malformed","refusal"]
    });
    let worker_failure = json!({
        "kind":"fail",
        "name":"worker_failed",
        "reason":"worker_failed"
    });
    json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":{"kind":"null"},
        "policy":{"policy":"policy.native-v2@1", "default":"deny"},
        "root":{
            "kind":"seq",
            "name":"root",
            "state":{"kind":"null"},
            "children":[
                {
                    "kind":"step",
                    "name":"worker",
                    "worker":"agent.worker@1",
                    "instructions":"Exercise the local worker.",
                    "input":{"kind":"null"},
                    "output":{"kind":"null"},
                    "inputBindings":[],
                    "writeBindings":[],
                    "timeoutMs":30000,
                    "attempts":1
                },
                {
                    "kind":"choice",
                    "name":"worker_result",
                    "state":{"kind":"null"},
                    "branches":[{
                        "when":worker_errors,
                        "node":worker_failure
                    }],
                    "otherwise":{
                        "kind":"succeed",
                        "name":"done",
                        "output":{"kind":"null"},
                        "bindings":[]
                    },
                    "promotedStatePaths":[]
                }
            ],
            "promotedStatePaths":[]
        }
    })
}
