use openengine_cluster_protocol::{
    admission_fingerprint, diff_compiled_graphs, CanonicalError, CompiledGraphIr, GraphIdentity,
};
use serde_json::{json, Value};

#[test]
fn attempts_per_node_keys_obey_node_name_bounds_in_rust_and_schema() {
    let mut value = serde_json::to_value(ir(&["a"], &["one"])).unwrap();
    let attempts = value["bounds"]["attemptsPerNode"].as_object_mut().unwrap();
    attempts.clear();
    attempts.insert("a".repeat(129), json!(1));
    assert!(serde_json::from_value::<CompiledGraphIr>(value.clone()).is_err());
    let schema = serde_json::to_value(schemars::schema_for!(CompiledGraphIr)).unwrap();
    assert!(!jsonschema::validator_for(&schema).unwrap().is_valid(&value));
}

fn ir(par_order: &[&str], seq_order: &[&str]) -> CompiledGraphIr {
    let terminal = |name: &str| {
        json!({
            "kind":"succeed", "name":name, "output":{"kind":"null"}, "bindings":[]
        })
    };
    let value = json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":{"kind":"record","fields":{}},
        "policy":{"policy":"policy.default@1","default":"deny"},
        "root":{
            "kind":"seq", "name":"root", "state":{"kind":"record","fields":{}},
            "children":[
                {
                    "kind":"par", "name":"parallel", "state":{"kind":"record","fields":{}},
                    "branches":par_order.iter().map(|name| terminal(name)).collect::<Vec<Value>>(),
                    "promotedStatePaths":[],
                    "join":{"kind":"first","when":{"kind":"all","guards":[
                        {"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},"labels":["accepted"]},
                        {"kind":"all","guards":[{"kind":"in","value":{"name":"verify","source":"error","field":null},"labels":["refusal"]}]}
                    ]}}
                },
                {
                    "kind":"seq", "name":"ordered", "state":{"kind":"record","fields":{}},
                    "children":seq_order.iter().map(|name| terminal(name)).collect::<Vec<Value>>(),
                    "promotedStatePaths":[]
                }
            ],
            "promotedStatePaths":[]
        },
        "bounds":{
            "termination":{"kind":"acyclic","order":["root","parallel","ordered"]},
            "maxNodeExecutions":8,
            "peakConcurrency":2,
            "attemptsPerNode":{"ordered":1,"parallel":1}
        }
    });
    serde_json::from_value(value).unwrap()
}

#[test]
fn canonical_ir_ignores_set_parallel_and_commutative_guard_order() {
    let left = ir(&["b", "a"], &["one", "two"]);
    let right = ir(&["a", "b"], &["one", "two"]);
    assert_eq!(
        left.canonical_bytes().unwrap(),
        right.canonical_bytes().unwrap()
    );
    assert_eq!(left.identity().unwrap(), right.identity().unwrap());

    let mut guard_reordered = serde_json::to_value(&left).unwrap();
    guard_reordered["root"]["children"][0]["join"]["when"]["guards"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let guard_reordered: CompiledGraphIr = serde_json::from_value(guard_reordered).unwrap();
    assert_eq!(
        left.identity().unwrap(),
        guard_reordered.identity().unwrap()
    );
}

#[test]
fn canonical_ir_recursively_sorts_every_object_key() {
    let bytes = ir(&["a", "b"], &["one", "two"]).canonical_bytes().unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.starts_with("{\"bounds\":"),
        "top-level keys were not sorted: {text}"
    );
    assert!(
        text.contains("\"attemptsPerNode\":{\"ordered\":1,\"parallel\":1}"),
        "nested map keys were not sorted: {text}"
    );
    assert!(
        text.contains(
            "{\"bindings\":[],\"kind\":\"succeed\",\"name\":\"a\",\"output\":{\"kind\":\"null\"}}"
        ),
        "nested struct keys were not sorted: {text}"
    );
}

#[test]
fn canonical_identity_changes_when_semantic_sequence_or_contract_changes() {
    let baseline = ir(&["a", "b"], &["one", "two"]);
    let reordered = ir(&["a", "b"], &["two", "one"]);
    assert_ne!(baseline.identity().unwrap(), reordered.identity().unwrap());

    let mut changed = baseline.clone();
    changed.policy.policy = "policy.changed@2".parse().unwrap();
    assert_ne!(baseline.identity().unwrap(), changed.identity().unwrap());

    changed = baseline.clone();
    changed.bounds.peak_concurrency = openengine_cluster_protocol::PositiveInteger::new(3).unwrap();
    assert_ne!(baseline.identity().unwrap(), changed.identity().unwrap());

    let mut changed_value = serde_json::to_value(&baseline).unwrap();
    changed_value["initialInput"] = json!({"kind":"string"});
    let changed_type: CompiledGraphIr = serde_json::from_value(changed_value).unwrap();
    assert_ne!(
        baseline.identity().unwrap(),
        changed_type.identity().unwrap()
    );

    let mut changed_value = serde_json::to_value(&baseline).unwrap();
    changed_value["root"]["children"][0]["branches"][0]["bindings"] = json!([{
        "target":["value"], "value":{"source":"state","path":["value"]}
    }]);
    let changed_binding: CompiledGraphIr = serde_json::from_value(changed_value).unwrap();
    assert_ne!(
        baseline.identity().unwrap(),
        changed_binding.identity().unwrap()
    );

    assert_ne!(
        step_ir("worker.impl@1", false).identity().unwrap(),
        step_ir("worker.impl@2", false).identity().unwrap()
    );
}

#[test]
fn graph_identity_is_sha256_and_round_trip_safe() {
    let identity = ir(&["a", "b"], &["one", "two"]).identity().unwrap();
    assert_eq!(identity.as_str().len(), 64);
    assert!(
        identity
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    let parsed: GraphIdentity = identity.to_string().parse().unwrap();
    assert_eq!(parsed, identity);
    assert!("A".repeat(64).parse::<GraphIdentity>().is_err());
}

fn step_ir(worker: &str, reverse_bindings: bool) -> CompiledGraphIr {
    let mut bindings = vec![
        json!({"target":["a"],"value":{"source":"state","path":["a"]}}),
        json!({"target":["b"],"value":{"source":"state","path":["b"]}}),
    ];
    if reverse_bindings {
        bindings.reverse();
    }
    serde_json::from_value(json!({
        "profile":"openengine.graph.single-worker/v1",
        "initialInput":{"kind":"record","fields":{}},
        "policy":{"policy":"policy.default@1","default":"deny"},
        "root":{
            "kind":"step","name":"work","worker":worker,
            "input":{"kind":"record","fields":{}},"output":{"kind":"null"},
            "inputBindings":bindings,"writeBindings":[],"timeoutMs":1000,"attempts":1
        },
        "bounds":{
            "termination":{"kind":"acyclic","order":["work"]},
            "maxNodeExecutions":1,"peakConcurrency":1,"attemptsPerNode":{"work":1}
        }
    }))
    .unwrap()
}

#[test]
fn canonical_ir_sorts_bindings_but_preserves_binding_content() {
    assert_eq!(
        step_ir("worker.impl@1", false).identity().unwrap(),
        step_ir("worker.impl@1", true).identity().unwrap()
    );
}

#[test]
fn canonical_ir_preserves_duplicate_binding_and_selector_multiplicity() {
    let baseline = step_ir("worker.impl@1", false);

    let mut duplicate_input = serde_json::to_value(&baseline).unwrap();
    let inputs = duplicate_input["root"]["inputBindings"]
        .as_array_mut()
        .unwrap();
    inputs.push(inputs[0].clone());
    let duplicate_input: CompiledGraphIr = serde_json::from_value(duplicate_input).unwrap();
    assert_ne!(
        baseline.identity().unwrap(),
        duplicate_input.identity().unwrap()
    );

    let write = json!({
        "value":{"node":"work","channel":"out","path":["value"]},
        "target":["value"]
    });
    let mut one_write = serde_json::to_value(&baseline).unwrap();
    one_write["root"]["writeBindings"] = json!([write.clone()]);
    let one_write: CompiledGraphIr = serde_json::from_value(one_write).unwrap();
    let mut duplicate_write = serde_json::to_value(&one_write).unwrap();
    duplicate_write["root"]["writeBindings"] = json!([write.clone(), write]);
    let duplicate_write: CompiledGraphIr = serde_json::from_value(duplicate_write).unwrap();
    assert_ne!(
        one_write.identity().unwrap(),
        duplicate_write.identity().unwrap()
    );

    let succeed_value = json!({
        "profile":"openengine.graph.single-worker/v1",
        "initialInput":{"kind":"record","fields":{}},
        "policy":{"policy":"policy.default@1","default":"deny"},
        "root":{
            "kind":"succeed","name":"done","output":{"kind":"string"},
            "bindings":[{
                "target":["value"],"value":{"source":"state","path":["value"]}
            }]
        },
        "bounds":{
            "termination":{"kind":"acyclic","order":["done"]},
            "maxNodeExecutions":1,"peakConcurrency":1,"attemptsPerNode":{"done":1}
        }
    });
    let succeed: CompiledGraphIr = serde_json::from_value(succeed_value.clone()).unwrap();
    let mut duplicate_succeed = succeed_value;
    let bindings = duplicate_succeed["root"]["bindings"]
        .as_array_mut()
        .unwrap();
    bindings.push(bindings[0].clone());
    let duplicate_succeed: CompiledGraphIr = serde_json::from_value(duplicate_succeed).unwrap();
    assert_ne!(
        succeed.identity().unwrap(),
        duplicate_succeed.identity().unwrap()
    );

    let selector = json!({"name":"verify","source":"signal","field":"verdict"});
    let mut one_selector = serde_json::to_value(ir(&["a", "b"], &["one", "two"])).unwrap();
    one_selector["root"]["children"][0]["join"]["when"] = json!({
        "kind":"k_of_n","count":1,"values":[selector.clone()],"labels":["accepted"]
    });
    let one_selector: CompiledGraphIr = serde_json::from_value(one_selector).unwrap();
    let mut duplicate_selector = serde_json::to_value(&one_selector).unwrap();
    duplicate_selector["root"]["children"][0]["join"]["when"]["values"] =
        json!([selector.clone(), selector]);
    let duplicate_selector: CompiledGraphIr = serde_json::from_value(duplicate_selector).unwrap();
    assert_ne!(
        one_selector.identity().unwrap(),
        duplicate_selector.identity().unwrap()
    );
}

#[test]
fn admission_fingerprint_sorts_json_keys_and_binds_the_method() {
    let left = admission_fingerprint(
        "apply",
        &json!({"input":{"z":1,"a":[true,null]},"dryRun":false}),
    )
    .unwrap();
    let reordered = admission_fingerprint(
        "apply",
        &json!({"dryRun":false,"input":{"a":[true,null],"z":1}}),
    )
    .unwrap();
    assert_eq!(left, reordered);
    assert_ne!(
        left,
        admission_fingerprint(
            "plan",
            &json!({"dryRun":false,"input":{"a":[true,null],"z":1}})
        )
        .unwrap()
    );
}

#[test]
fn compiled_node_diff_is_sorted_and_rejects_duplicate_names() {
    let baseline = ir(&["a", "b"], &["one", "two"]);
    let created = diff_compiled_graphs(None, &baseline).unwrap();
    let created_names: Vec<_> = created.added.iter().map(|name| name.as_str()).collect();
    assert_eq!(
        created_names,
        ["a", "b", "one", "ordered", "parallel", "root", "two"]
    );

    let mut duplicate = serde_json::to_value(&baseline).unwrap();
    duplicate["root"]["children"][1]["children"][1]["name"] = json!("one");
    let duplicate: CompiledGraphIr = serde_json::from_value(duplicate).unwrap();
    assert!(matches!(
        diff_compiled_graphs(None, &duplicate),
        Err(CanonicalError::DuplicateNodeName(name)) if name.as_str() == "one"
    ));
}
