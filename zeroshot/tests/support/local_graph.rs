use serde_json::{Value, json};

pub(super) fn graph() -> Value {
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
                {"kind":"succeed", "name":"done", "output":{"kind":"null"}, "bindings":[]}
            ],
            "promotedStatePaths":[]
        }
    })
}
