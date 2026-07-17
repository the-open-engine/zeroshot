use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    GraphDiagnosticCode, GraphProfile, GraphSpec, TerminationWitness, WorkerDescriptor, WorkerRef,
};
use openengine_cluster_server::admission::{GraphVerifier, VerificationError};
use openengine_cluster_server::graph_verifier::ProductionGraphVerifier;
use openengine_cluster_server::worker_registry::{WorkerRegistry, WorkerRegistryError};
use serde_json::{json, Value};

#[derive(Clone)]
struct MemoryRegistry {
    descriptors: Arc<BTreeMap<WorkerRef, WorkerDescriptor>>,
    resolutions: Arc<AtomicUsize>,
}

struct VersionUnavailableRegistry;

#[async_trait]
impl WorkerRegistry for VersionUnavailableRegistry {
    async fn resolve(&self, worker: &WorkerRef) -> Result<WorkerDescriptor, WorkerRegistryError> {
        Err(WorkerRegistryError::VersionUnavailable {
            worker: worker.clone(),
        })
    }
}

#[async_trait]
impl WorkerRegistry for MemoryRegistry {
    async fn resolve(&self, worker: &WorkerRef) -> Result<WorkerDescriptor, WorkerRegistryError> {
        self.resolutions.fetch_add(1, Ordering::Relaxed);
        self.descriptors
            .get(worker)
            .cloned()
            .ok_or_else(|| WorkerRegistryError::NotFound {
                worker: worker.clone(),
            })
    }
}

fn record() -> Value {
    json!({
        "kind": "record",
        "fields": {
            "value": { "type": { "kind": "integer" }, "required": true },
            "result": { "type": { "kind": "number" }, "required": false },
            "verdict": { "type": { "kind": "enum", "values": ["accepted", "rejected"] }, "required": false },
            "diagnostic": { "type": { "kind": "number" }, "required": false }
        }
    })
}

fn descriptor(worker: &str, verifier: bool) -> WorkerDescriptor {
    serde_json::from_value(json!({
        "worker": worker,
        "graphProfiles": ["openengine.graph.full/v1"],
        "binding": { "protocol": "acp", "version": "1", "profile": "openengine.worker.acp/v1" },
        "contract": {
            "input": if verifier { json!({"kind":"null"}) } else { record() },
            "output": {"kind":"record","fields":{"result":{"type":{"kind":"integer"},"required":true}}},
            "verifier": if verifier { json!({
                "signals": { "verdict": ["accepted", "rejected"] },
                "diagnostic": { "kind": "record", "fields": {
                    "code": {"type":{"kind":"integer"},"required":true}
                } }
            }) } else { Value::Null },
            "errors": ["timeout", "crash", "malformed", "refusal"]
        },
        "capabilityPolicy": { "autonomy": "strict", "permissionPolicy": "policy.strict@1" },
        "artifactProfile": {
            "allowedTypeIds": ["openengine.result@1"],
            "allowedMediaTypes": ["application/json"],
            "minimumRedaction": "internal"
        },
        "credentialRequirements": []
    }))
    .unwrap()
}

fn registry() -> MemoryRegistry {
    MemoryRegistry {
        descriptors: Arc::new(BTreeMap::from([
            (
                WorkerRef::new("worker.main@1").unwrap(),
                descriptor("worker.main@1", false),
            ),
            (
                WorkerRef::new("worker.verify@1").unwrap(),
                descriptor("worker.verify@1", true),
            ),
        ])),
        resolutions: Arc::new(AtomicUsize::new(0)),
    }
}

fn valid_graph() -> Value {
    json!({
        "profile": "openengine.graph.full/v1",
        "initialInput": record(),
        "policy": { "policy": "policy.strict@1", "default": "deny" },
        "root": {
            "kind": "seq", "name": "root", "state": record(),
            "children": [
                {
                    "kind": "step", "name": "work", "worker": "worker.main@1",
                    "input": record(),
                    "output": {"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}},
                    "inputBindings": [{"target":["value"],"value":{"source":"state","path":["value"]}}],
                    "writeBindings": [{
                        "value":{"node":"work","channel":"out","path":["result"]},
                        "target":["result"]
                    }],
                    "timeoutMs": 172800000, "attempts": 2
                },
                {
                    "kind": "verifier", "name": "verify", "worker": "worker.verify@1",
                    "input": {"kind":"null"}, "output": {"kind":"record","fields":{}},
                    "inputBindings": [], "writeBindings": [], "timeoutMs": 1, "attempts": 1,
                    "signals": {"verdict":["accepted","rejected"]},
                    "diagnostic": {"kind":"record","fields":{}}
                },
                {
                    "kind":"choice", "name":"decision", "state":record(),
                    "branches":[{
                        "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},"labels":["accepted"]},
                        "node":{"kind":"succeed","name":"accepted","output":record(),"bindings":[
                            {"target":["value"],"value":{"source":"state","path":["value"]}}
                        ]}
                    }],
                    "otherwise":{"kind":"fail","name":"rejected","reason":"rejected"},
                    "promotedStatePaths":[]
                }
            ],
            "promotedStatePaths":[]
        }
    })
}

#[tokio::test]
async fn verifies_full_graph_and_returns_byte_stable_authoritative_ir() {
    let graph: GraphSpec = serde_json::from_value(valid_graph()).unwrap();
    let verifier = ProductionGraphVerifier::new(registry());
    let first = verifier.verify(&graph).await.unwrap();
    let second = verifier.verify(&graph).await.unwrap();

    assert_eq!(first, second);
    assert_eq!(first.compiled_ir.profile, graph.profile);
    assert_eq!(first.compiled_ir.initial_input, graph.initial_input);
    assert_eq!(first.compiled_ir.policy, graph.policy);
    assert_eq!(first.compiled_ir.root, graph.root);
    assert_eq!(first.compiled_ir.bounds.max_node_executions.get(), 2);
    assert_eq!(first.compiled_ir.bounds.peak_concurrency.get(), 1);
    assert!(matches!(
        first.compiled_ir.bounds.termination,
        TerminationWitness::Acyclic { .. }
    ));
    assert_eq!(
        first.compiled_ir.canonical_bytes().unwrap(),
        second.compiled_ir.canonical_bytes().unwrap()
    );
}

#[tokio::test]
async fn structural_rejection_precedes_registry_resolution_and_is_stably_sorted() {
    let mut value = valid_graph();
    value["root"]["children"][2] = json!({
        "kind":"step", "name":"work", "worker":"missing.worker@1",
        "input":{"kind":"null"}, "output":{"kind":"null"},
        "inputBindings":[], "writeBindings":[], "timeoutMs":1, "attempts":1
    });
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    let registry = registry();
    let resolutions = Arc::clone(&registry.resolutions);
    let verifier = ProductionGraphVerifier::new(registry);

    let first = verifier.verify(&graph).await.unwrap_err();
    let second = verifier.verify(&graph).await.unwrap_err();
    assert_eq!(first, second);
    assert_eq!(resolutions.load(Ordering::Relaxed), 0);
    let VerificationError::Rejected { diagnostics } = first else {
        panic!("structural invalidity must be rejected")
    };
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == GraphDiagnosticCode::InvalidGraphShape)
    );
}

#[tokio::test]
async fn single_worker_profile_and_missing_worker_are_rejections() {
    let mut single = valid_graph();
    single["profile"] = json!("openengine.graph.single-worker/v1");
    let single: GraphSpec = serde_json::from_value(single).unwrap();
    assert_eq!(single.profile, GraphProfile::SingleWorker);
    assert!(matches!(
        ProductionGraphVerifier::new(registry())
            .verify(&single)
            .await,
        Err(VerificationError::Rejected { .. })
    ));

    let mut missing = valid_graph();
    missing["root"]["children"][0]["worker"] = json!("worker.missing@2");
    let missing: GraphSpec = serde_json::from_value(missing).unwrap();
    let error = ProductionGraphVerifier::new(registry())
        .verify(&missing)
        .await
        .unwrap_err();
    assert!(matches!(error, VerificationError::Rejected { .. }));

    let graph: GraphSpec = serde_json::from_value(valid_graph()).unwrap();
    let version_error = ProductionGraphVerifier::new(VersionUnavailableRegistry)
        .verify(&graph)
        .await
        .unwrap_err();
    assert!(matches!(version_error, VerificationError::Rejected { .. }));
}

#[test]
fn timeout_wire_range_has_no_product_ceiling() {
    let graph: GraphSpec = serde_json::from_value(valid_graph()).unwrap();
    let Value::Number(timeout) =
        serde_json::to_value(graph).unwrap()["root"]["children"][0]["timeoutMs"].clone()
    else {
        panic!("timeout must serialize as an integer")
    };
    assert_eq!(timeout.as_u64(), Some(172_800_000));

    for invalid in [0_u64, openengine_cluster_protocol::MAX_SAFE_GENERATION + 1] {
        let mut value = valid_graph();
        value["root"]["children"][0]["timeoutMs"] = json!(invalid);
        assert!(serde_json::from_value::<GraphSpec>(value).is_err());
    }
}

fn rejection_codes(error: VerificationError) -> Vec<GraphDiagnosticCode> {
    let VerificationError::Rejected { diagnostics } = error else {
        panic!("invalid graph must be rejected")
    };
    diagnostics
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

fn required_empty_record_payload() -> Value {
    json!({
        "kind": "record",
        "fields": {
            "object": {
                "type": { "kind": "record", "fields": {} },
                "required": true
            }
        }
    })
}

#[tokio::test]
async fn required_empty_record_input_must_be_bound() {
    let mut value = valid_graph();
    value["root"]["children"][0]["input"] = required_empty_record_payload();
    value["root"]["children"][0]["inputBindings"] = json!([]);
    let graph: GraphSpec = serde_json::from_value(value).unwrap();

    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();

    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::UndefinedRead));
}

#[tokio::test]
async fn required_empty_record_succeed_output_must_be_bound() {
    let mut value = valid_graph();
    value["root"]["children"][2]["branches"][0]["node"]["output"] = required_empty_record_payload();
    value["root"]["children"][2]["branches"][0]["node"]["bindings"] = json!([]);
    let graph: GraphSpec = serde_json::from_value(value).unwrap();

    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();

    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::UndefinedRead));
}

#[tokio::test]
async fn negative_semantic_matrix_rejects_undefined_reads_types_choices_and_quorum() {
    let mut undefined = valid_graph();
    undefined["root"]["children"][0]["inputBindings"][0]["value"]["path"] = json!(["result"]);
    let undefined: GraphSpec = serde_json::from_value(undefined).unwrap();
    assert!(
        rejection_codes(
            ProductionGraphVerifier::new(registry())
                .verify(&undefined)
                .await
                .unwrap_err()
        )
        .contains(&GraphDiagnosticCode::UndefinedRead)
    );

    let mut mismatch = valid_graph();
    mismatch["root"]["children"][0]["input"]["fields"]["value"]["type"] = json!({"kind":"string"});
    let mismatch: GraphSpec = serde_json::from_value(mismatch).unwrap();
    assert!(
        rejection_codes(
            ProductionGraphVerifier::new(registry())
                .verify(&mismatch)
                .await
                .unwrap_err()
        )
        .contains(&GraphDiagnosticCode::SchemaSafety)
    );

    let mut non_exhaustive = valid_graph();
    non_exhaustive["root"]["children"][2]["otherwise"] = Value::Null;
    let non_exhaustive: GraphSpec = serde_json::from_value(non_exhaustive).unwrap();
    assert!(
        rejection_codes(
            ProductionGraphVerifier::new(registry())
                .verify(&non_exhaustive)
                .await
                .unwrap_err()
        )
        .contains(&GraphDiagnosticCode::ChoiceExhaustiveness)
    );

    let mut dead = valid_graph();
    let first = dead["root"]["children"][2]["branches"][0].clone();
    let mut second = first.clone();
    second["node"]["name"] = json!("deadBranch");
    dead["root"]["children"][2]["branches"] = json!([first, second]);
    let dead: GraphSpec = serde_json::from_value(dead).unwrap();
    assert!(
        rejection_codes(
            ProductionGraphVerifier::new(registry())
                .verify(&dead)
                .await
                .unwrap_err()
        )
        .contains(&GraphDiagnosticCode::ChoiceExhaustiveness)
    );

    let mut invalid_quorum = valid_graph();
    let branch = invalid_quorum["root"]["children"][0].clone();
    let mut other = branch.clone();
    other["name"] = json!("otherWork");
    invalid_quorum["root"]["children"][2] = json!({
        "kind":"seq","name":"tail","state":record(),
        "children":[
            {"kind":"par","name":"parallel","state":record(),"branches":[branch,other],
             "promotedStatePaths":[],"join":{"kind":"quorum","count":3}},
            {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
        ],
        "promotedStatePaths":[]
    });
    let invalid_quorum: GraphSpec = serde_json::from_value(invalid_quorum).unwrap();
    assert!(
        rejection_codes(
            ProductionGraphVerifier::new(registry())
                .verify(&invalid_quorum)
                .await
                .unwrap_err()
        )
        .contains(&GraphDiagnosticCode::InvalidGraphShape)
    );
}

#[tokio::test]
async fn loop_exit_parallel_write_and_promotion_safety_fail_closed() {
    let verifier = valid_graph()["root"]["children"][1].clone();
    let contradictory = json!({
        "kind":"all","guards":[
            {"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},"labels":["accepted"]},
            {"kind":"not","guard":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},"labels":["accepted"]}}
        ]
    });
    let loop_graph = graph_with_root_child(json!({
        "kind":"seq","name":"loopTail","state":record(),"children":[
            {"kind":"loop","name":"loop","state":record(),"body":verifier,
             "until":contradictory,"maxIterations":2,"promotedStatePaths":[]},
            {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
        ],"promotedStatePaths":[]
    }));
    assert!(
        rejection_codes(
            ProductionGraphVerifier::new(registry())
                .verify(&loop_graph)
                .await
                .unwrap_err()
        )
        .contains(&GraphDiagnosticCode::LoopExitSatisfiability)
    );

    let mut left = valid_graph()["root"]["children"][0].clone();
    left["name"] = json!("left");
    left["writeBindings"][0]["value"]["node"] = json!("left");
    let mut right = left.clone();
    right["name"] = json!("right");
    right["writeBindings"][0]["value"]["node"] = json!("right");
    let conflict = graph_with_root_child(json!({
        "kind":"seq","name":"parallelTail","state":record(),"children":[
            {"kind":"par","name":"parallel","state":record(),"branches":[left,right],
             "promotedStatePaths":[],"join":{"kind":"all"}},
            {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
        ],"promotedStatePaths":[]
    }));
    assert!(
        rejection_codes(
            ProductionGraphVerifier::new(registry())
                .verify(&conflict)
                .await
                .unwrap_err()
        )
        .contains(&GraphDiagnosticCode::WriteConflict)
    );

    let work = valid_graph()["root"]["children"][0].clone();
    let unsafe_promotion = graph_with_root_child(json!({
        "kind":"seq","name":"choiceTail","state":record(),"children":[
            {"kind":"verifier","name":"verify","worker":"worker.verify@1",
             "input":{"kind":"null"},"output":{"kind":"record","fields":{}},
             "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
             "signals":{"verdict":["accepted","rejected"]},"diagnostic":{"kind":"record","fields":{}}},
            {"kind":"choice","name":"promote","state":record(),"branches":[{
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},"labels":["accepted"]},
                "node":work
             }],"otherwise":{"kind":"fail","name":"failed","reason":"failed"},
             "promotedStatePaths":[["result"]]},
            {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
        ],"promotedStatePaths":[]
    }));
    assert!(
        rejection_codes(
            ProductionGraphVerifier::new(registry())
                .verify(&unsafe_promotion)
                .await
                .unwrap_err()
        )
        .contains(&GraphDiagnosticCode::UndefinedRead)
    );
}

#[tokio::test]
async fn cyclic_node_output_references_are_rejected() {
    let mut left = valid_graph()["root"]["children"][0].clone();
    left["name"] = json!("left");
    left["writeBindings"][0]["value"]["node"] = json!("right");
    let mut right = valid_graph()["root"]["children"][0].clone();
    right["name"] = json!("right");
    right["writeBindings"][0]["value"]["node"] = json!("left");
    let graph = graph_with_root_child(json!({
        "kind":"seq","name":"root","state":record(),
        "children":[left,right,{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}],
        "promotedStatePaths":[]
    }));
    let codes = rejection_codes(
        ProductionGraphVerifier::new(registry())
            .verify(&graph)
            .await
            .unwrap_err(),
    );
    assert!(codes.contains(&GraphDiagnosticCode::CyclicReference));
    assert!(codes.contains(&GraphDiagnosticCode::UndefinedRead));
}

fn graph_with_root_child(child: Value) -> GraphSpec {
    serde_json::from_value(json!({
        "profile":"openengine.graph.full/v1","initialInput":record(),
        "policy":{"policy":"policy.strict@1","default":"deny"},"root":child
    }))
    .unwrap()
}

#[tokio::test]
async fn every_guard_form_and_map_aggregate_is_admitted_when_satisfiable() {
    for guard in [
        json!({"kind":"any","guards":[
            {"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},"labels":["accepted"]},
            {"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},"labels":["rejected"]}
        ]}),
        json!({"kind":"not","guard":
            {"kind":"in","value":{"name":"verify","source":"error","field":null},"labels":["timeout"]}
        }),
        json!({"kind":"k_of_n","count":1,"values":[
            {"name":"verify","source":"signal","field":"verdict"},
            {"name":"verify","source":"error","field":null}
        ],"labels":["accepted","timeout"]}),
    ] {
        let mut value = valid_graph();
        value["root"]["children"][2]["branches"][0]["when"] = guard;
        let graph: GraphSpec = serde_json::from_value(value).unwrap();
        ProductionGraphVerifier::new(registry())
            .verify(&graph)
            .await
            .unwrap();
    }

    let map_graph: GraphSpec = serde_json::from_value(json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":{"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}},
        "policy":{"policy":"policy.strict@1","default":"deny"},
        "root":{"kind":"seq","name":"root","state":{"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}},
        "children":[
            {"kind":"map","name":"map","state":{"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}},
             "body":{"kind":"verifier","name":"mapVerify","worker":"worker.verify@1",
                "input":{"kind":"null"},"output":{"kind":"record","fields":{}},
                "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
                "signals":{"verdict":["accepted","rejected"]},"diagnostic":{"kind":"record","fields":{}}},
             "over":{"source":"state","path":["items"]},"maxItems":2,"promotedStatePaths":[]},
            {"kind":"choice","name":"choice","state":{"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}},
             "branches":[
                {"when":{"kind":"k_of_map","count":2,
                    "value":{"name":"mapVerify","source":"signal","field":"verdict"},"labels":["accepted"]},
                    "node":{"kind":"succeed","name":"selectedTwice","output":{"kind":"null"},"bindings":[]}},
                {"when":{"kind":"k_of_map","count":1,
                    "value":{"name":"mapVerify","source":"signal","field":"verdict"},"labels":["accepted"]},
                    "node":{"kind":"succeed","name":"selectedOnce","output":{"kind":"null"},"bindings":[]}}
             ],
             "otherwise":{"kind":"fail","name":"failed","reason":"failed"},"promotedStatePaths":[]}
        ],"promotedStatePaths":[]}
    }))
    .unwrap();
    ProductionGraphVerifier::new(registry())
        .verify(&map_graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn exhaustive_terminal_choice_without_otherwise_is_admitted() {
    let mut value = valid_graph();
    value["root"]["children"][2] = json!({
        "kind":"choice", "name":"decision", "state":record(),
        "branches":[
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["accepted","rejected"]},
                "node":{"kind":"succeed","name":"completed","output":{"kind":"null"},"bindings":[]}
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"failed","reason":"worker_error"}
            }
        ],
        "otherwise":null,
        "promotedStatePaths":[]
    });
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn exhaustive_terminal_choice_rejects_dead_otherwise() {
    let mut value = valid_graph();
    value["root"]["children"][2] = json!({
        "kind":"choice", "name":"decision", "state":record(),
        "branches":[
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["accepted"]},
                "node":{"kind":"succeed","name":"accepted","output":{"kind":"null"},"bindings":[]}
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["rejected"]},
                "node":{"kind":"fail","name":"rejected","reason":"rejected"}
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"workerFailed","reason":"worker_error"}
            }
        ],
        "otherwise":{
            "kind":"succeed","name":"deadOtherwise","output":record(),
            "bindings":[{"target":["value"],"value":{"source":"state","path":["missing"]}}]
        },
        "promotedStatePaths":[]
    });
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();
    let VerificationError::Rejected { diagnostics } = error else {
        panic!("dead otherwise must be rejected")
    };
    assert_eq!(
        diagnostics.len(),
        1,
        "dead otherwise must not enter flow analysis"
    );
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.code == GraphDiagnosticCode::ChoiceExhaustiveness
            && serde_json::to_value(&diagnostic.path).unwrap() == dead_otherwise_diagnostic_path()
    }));
}

#[tokio::test]
async fn dead_nonterminal_otherwise_does_not_cause_terminal_fallthrough() {
    let mut value = valid_graph();
    value["root"]["children"][2] = json!({
        "kind":"choice", "name":"decision", "state":record(),
        "branches":[
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["accepted","rejected"]},
                "node":{"kind":"succeed","name":"completed","output":{"kind":"null"},"bindings":[]}
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"workerFailed","reason":"worker_error"}
            }
        ],
        "otherwise":{
            "kind":"step", "name":"deadOtherwise", "worker":"worker.main@1",
            "input":{"kind":"null"}, "output":{"kind":"null"},
            "inputBindings":[], "writeBindings":[], "timeoutMs":1, "attempts":1
        },
        "promotedStatePaths":[]
    });
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();
    let VerificationError::Rejected { diagnostics } = error else {
        panic!("dead otherwise must be rejected")
    };
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code,
        GraphDiagnosticCode::ChoiceExhaustiveness
    );
    assert_eq!(
        serde_json::to_value(&diagnostics[0].path).unwrap(),
        dead_otherwise_diagnostic_path()
    );
}

#[tokio::test]
async fn dead_nonterminal_guarded_branch_does_not_cause_terminal_fallthrough() {
    let mut value = valid_graph();
    value["root"]["children"][2] = json!({
        "kind":"choice", "name":"decision", "state":record(),
        "branches":[
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["accepted","rejected"]},
                "node":{"kind":"succeed","name":"completed","output":{"kind":"null"},"bindings":[]}
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":["accepted"]},
                "node":{
                    "kind":"step", "name":"deadBranch", "worker":"worker.main@1",
                    "input":record(), "output":{"kind":"null"},
                    "inputBindings":[{"target":["value"],"value":{"source":"state","path":["missing"]}}],
                    "writeBindings":[], "timeoutMs":1, "attempts":1
                }
            },
            {
                "when":{"kind":"in","value":{"name":"verify","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"workerFailed","reason":"worker_error"}
            }
        ],
        "otherwise":null,
        "promotedStatePaths":[]
    });
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();
    let VerificationError::Rejected { diagnostics } = error else {
        panic!("dead guarded branch must be rejected")
    };
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code,
        GraphDiagnosticCode::ChoiceExhaustiveness
    );
    assert_eq!(
        diagnostics[0].message,
        "choice branch is unreachable after excluding earlier branches"
    );
}

fn dead_otherwise_diagnostic_path() -> Value {
    json!([
        {"kind":"field","name":"root"},
        {"kind":"node","name":"root"},
        {"kind":"field","name":"children"},
        {"kind":"index","index":2},
        {"kind":"node","name":"decision"},
        {"kind":"field","name":"otherwise"},
        {"kind":"node","name":"deadOtherwise"}
    ])
}

#[tokio::test]
async fn choice_residual_outcomes_protect_unavailable_verifier_outputs() {
    let mut value = valid_graph();
    value["root"]["children"][1]["output"] =
        json!({"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}});
    value["root"]["children"][2] = json!({
        "kind":"choice", "name":"decision", "state":record(),
        "branches":[{
            "when":{"kind":"not","guard":{"kind":"in",
                "value":{"name":"verify","source":"signal","field":"verdict"},"labels":["accepted"]}},
            "node":{
                "kind":"step", "name":"recover", "worker":"worker.main@1",
                "input":{"kind":"null"}, "output":{"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}},
                "inputBindings":[],
                "writeBindings":[{"value":{"node":"verify","channel":"out","path":["result"]},"target":["result"]}],
                "timeoutMs":1, "attempts":1
            }
        }],
        "otherwise":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]},
        "promotedStatePaths":[]
    });
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();
    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::UndefinedRead));
}

#[tokio::test]
async fn terminal_error_paths_do_not_poison_success_only_continuations() {
    let mut value = valid_graph();
    value["root"]["children"][1]["output"] =
        json!({"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}});
    value["root"]["children"][2] = json!({
        "kind":"choice", "name":"routeError", "state":record(),
        "branches":[{
            "when":{"kind":"in","value":{"name":"verify","source":"error","field":null},
                "labels":["timeout","crash","malformed","refusal"]},
            "node":{"kind":"fail","name":"workerFailed","reason":"worker_error"}
        }],
        "otherwise":{
            "kind":"step", "name":"consume", "worker":"worker.main@1",
            "input":record(), "output":{"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}},
            "inputBindings":[{"target":["value"],"value":{"source":"state","path":["value"]}}],
            "writeBindings":[{"value":{"node":"verify","channel":"out","path":["result"]},"target":["result"]}],
            "timeoutMs":1, "attempts":1
        },
        "promotedStatePaths":[]
    });
    value["root"]["children"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]
        }));
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn output_backed_writes_are_undefined_until_success_is_guaranteed() {
    let mut value = valid_graph();
    value["root"]["children"] = json!([
        value["root"]["children"][0].clone(),
        {
            "kind":"succeed", "name":"done",
            "output":{"kind":"record","fields":{
                "result":{"type":{"kind":"number"},"required":true}
            }},
            "bindings":[{
                "target":["result"],
                "value":{"source":"state","path":["result"]}
            }]
        }
    ]);
    let graph: GraphSpec = serde_json::from_value(value).unwrap();
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();

    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::UndefinedRead));
}

#[tokio::test]
async fn success_routing_makes_output_backed_writes_definite() {
    let mut value = valid_graph();
    value["root"]["children"] = json!([
        value["root"]["children"][0].clone(),
        {
            "kind":"choice", "name":"routeWork", "state":record(),
            "branches":[{
                "when":{"kind":"in",
                    "value":{"name":"work","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"workFailed","reason":"worker_error"}
            }],
            "otherwise":{
                "kind":"succeed", "name":"done",
                "output":{"kind":"record","fields":{
                    "result":{"type":{"kind":"number"},"required":true}
                }},
                "bindings":[{"target":["result"],
                    "value":{"source":"state","path":["result"]}}]
            },
            "promotedStatePaths":[]
        }
    ]);
    let graph: GraphSpec = serde_json::from_value(value).unwrap();

    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn required_initial_paths_survive_optional_group_state_widening() {
    let mut value = valid_graph();
    value["root"]["state"]["fields"]["value"]["required"] = json!(false);
    value["root"]["state"]["fields"]["value"]["type"] = json!({"kind":"number"});
    let graph: GraphSpec = serde_json::from_value(value).unwrap();

    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn success_routing_does_not_define_an_optional_output_path() {
    let mut value = valid_graph();
    value["root"]["children"][0]["output"]["fields"]["result"]["required"] = json!(false);
    value["root"]["children"] = json!([
        value["root"]["children"][0].clone(),
        {
            "kind":"choice", "name":"routeWork", "state":record(),
            "branches":[{
                "when":{"kind":"in",
                    "value":{"name":"work","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"workFailed","reason":"worker_error"}
            }],
            "otherwise":{
                "kind":"succeed", "name":"done",
                "output":{"kind":"record","fields":{
                    "result":{"type":{"kind":"number"},"required":true}
                }},
                "bindings":[{"target":["result"],
                    "value":{"source":"state","path":["result"]}}]
            },
            "promotedStatePaths":[]
        }
    ]);
    let graph: GraphSpec = serde_json::from_value(value).unwrap();

    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();

    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::UndefinedRead));
}

#[tokio::test]
async fn writing_a_required_record_does_not_define_its_optional_descendant() {
    let mut value = valid_graph();
    let state = json!({
        "kind":"record","fields":{
            "value":{"type":{"kind":"integer"},"required":true},
            "stored":{"type":{"kind":"record","fields":{
                "maybe":{"type":{"kind":"number"},"required":false}
            }},"required":false}
        }
    });
    let output = json!({
        "kind":"record","fields":{
            "payload":{"type":{"kind":"record","fields":{
                "maybe":{"type":{"kind":"number"},"required":false}
            }},"required":true}
        }
    });
    value["root"]["state"] = state.clone();
    value["root"]["children"][0]["output"] = output.clone();
    value["root"]["children"][0]["writeBindings"] = json!([{
        "value":{"node":"work","channel":"out","path":["payload"]},
        "target":["stored"]
    }]);
    value["root"]["children"] = json!([
        value["root"]["children"][0].clone(),
        {
            "kind":"choice", "name":"routeWork", "state":state,
            "branches":[{
                "when":{"kind":"in",
                    "value":{"name":"work","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"workFailed","reason":"worker_error"}
            }],
            "otherwise":{
                "kind":"succeed", "name":"done",
                "output":{"kind":"record","fields":{
                    "maybe":{"type":{"kind":"number"},"required":true}
                }},
                "bindings":[{"target":["maybe"],
                    "value":{"source":"state","path":["stored","maybe"]}}]
            },
            "promotedStatePaths":[]
        }
    ]);
    let graph: GraphSpec = serde_json::from_value(value).unwrap();

    let mut worker = serde_json::to_value(descriptor("worker.main@1", false)).unwrap();
    worker["contract"]["output"] = output;
    let registry = MemoryRegistry {
        descriptors: Arc::new(BTreeMap::from([(
            WorkerRef::new("worker.main@1").unwrap(),
            serde_json::from_value(worker).unwrap(),
        )])),
        resolutions: Arc::new(AtomicUsize::new(0)),
    };
    let error = ProductionGraphVerifier::new(registry)
        .verify(&graph)
        .await
        .unwrap_err();

    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::UndefinedRead));
}

#[tokio::test]
async fn success_routing_does_not_define_an_optional_diagnostic_path() {
    let mut value = valid_graph();
    value["root"]["children"][1]["diagnostic"] = json!({
        "kind":"record","fields":{
            "code":{"type":{"kind":"number"},"required":false}
        }
    });
    value["root"]["children"][1]["writeBindings"] = json!([{
        "value":{"node":"verify","channel":"diagnostic","path":["code"]},
        "target":["diagnostic"]
    }]);
    value["root"]["children"] = json!([
        value["root"]["children"][1].clone(),
        {
            "kind":"choice", "name":"routeVerify", "state":record(),
            "branches":[{
                "when":{"kind":"in",
                    "value":{"name":"verify","source":"error","field":null},
                    "labels":["timeout","crash","malformed","refusal"]},
                "node":{"kind":"fail","name":"verifyFailed","reason":"worker_error"}
            }],
            "otherwise":{
                "kind":"succeed", "name":"done",
                "output":{"kind":"record","fields":{
                    "diagnostic":{"type":{"kind":"number"},"required":true}
                }},
                "bindings":[{"target":["diagnostic"],
                    "value":{"source":"state","path":["diagnostic"]}}]
            },
            "promotedStatePaths":[]
        }
    ]);
    let graph: GraphSpec = serde_json::from_value(value).unwrap();

    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();

    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::UndefinedRead));
}

#[tokio::test]
async fn output_backed_promotions_require_success_guarantees() {
    let graph = graph_with_root_child(json!({
        "kind":"seq","name":"root","state":record(),"children":[
            {"kind":"seq","name":"promoteWork","state":record(),
             "children":[valid_graph()["root"]["children"][0].clone()],
             "promotedStatePaths":[["result"]]},
            {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
        ],"promotedStatePaths":[]
    }));
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();

    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::UndefinedRead));
}

#[tokio::test]
async fn promoted_writes_must_be_subtypes_of_the_enclosing_state() {
    let enclosing_state = json!({
        "kind":"record","fields":{
            "value":{"type":{"kind":"integer"},"required":true},
            "result":{"type":{"kind":"integer"},"required":true}
        }
    });
    let child_state = json!({
        "kind":"record","fields":{
            "value":{"type":{"kind":"integer"},"required":true},
            "result":{"type":{"kind":"number"},"required":true}
        }
    });
    let graph: GraphSpec = serde_json::from_value(json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":enclosing_state,
        "policy":{"policy":"policy.strict@1","default":"deny"},
        "root":{
            "kind":"seq","name":"root","state":enclosing_state,"children":[
                {"kind":"seq","name":"promoteNumber","state":child_state,"children":[
                    {"kind":"step","name":"numberWork","worker":"worker.main@1",
                     "input":child_state,
                     "output":{"kind":"record","fields":{
                         "result":{"type":{"kind":"number"},"required":true}
                     }},
                     "inputBindings":[
                         {"target":["value"],"value":{"source":"state","path":["value"]}},
                         {"target":["result"],"value":{"source":"state","path":["result"]}}
                     ],
                     "writeBindings":[{
                         "value":{"node":"numberWork","channel":"out","path":["result"]},
                         "target":["result"]
                     }],
                     "timeoutMs":1,"attempts":1}
                ],"promotedStatePaths":[["result"]]},
                {"kind":"choice","name":"routeNumberWork","state":enclosing_state,
                 "branches":[{
                     "when":{"kind":"in",
                         "value":{"name":"numberWork","source":"error","field":null},
                         "labels":["timeout","crash","malformed","refusal"]},
                     "node":{"kind":"fail","name":"numberWorkFailed","reason":"worker_error"}
                 }],
                 "otherwise":{"kind":"succeed","name":"done",
                     "output":{"kind":"record","fields":{
                         "result":{"type":{"kind":"integer"},"required":true}
                     }},
                     "bindings":[{
                         "target":["result"],"value":{"source":"state","path":["result"]}
                     }]},
                 "promotedStatePaths":[]}
            ],"promotedStatePaths":[]
        }
    }))
    .unwrap();
    let mut number_descriptor = serde_json::to_value(descriptor("worker.main@1", false)).unwrap();
    number_descriptor["contract"]["output"]["fields"]["result"]["type"] = json!({"kind":"number"});
    let number_registry = MemoryRegistry {
        descriptors: Arc::new(BTreeMap::from([(
            WorkerRef::new("worker.main@1").unwrap(),
            serde_json::from_value(number_descriptor).unwrap(),
        )])),
        resolutions: Arc::new(AtomicUsize::new(0)),
    };

    let error = ProductionGraphVerifier::new(number_registry)
        .verify(&graph)
        .await
        .unwrap_err();
    let VerificationError::Rejected { diagnostics } = error else {
        panic!("incompatible promotion must be rejected")
    };
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.code == GraphDiagnosticCode::SchemaSafety
                && diagnostic.message
                    == "promoted value is not a subtype of its enclosing state target"
                && serde_json::to_value(&diagnostic.path).unwrap()
                    == json!([
                        {"kind":"field","name":"root"},
                        {"kind":"node","name":"root"},
                        {"kind":"field","name":"children"},
                        {"kind":"index","index":0},
                        {"kind":"node","name":"promoteNumber"},
                        {"kind":"field","name":"promotedStatePaths"},
                        {"kind":"index","index":0}
                    ])
        }),
        "unexpected diagnostics: {diagnostics:#?}"
    );

    let mut safe_graph = serde_json::to_value(&graph).unwrap();
    safe_graph["root"]["children"][0]["children"][0]["output"]["fields"]["result"]["type"] =
        json!({"kind":"integer"});
    let safe_graph: GraphSpec = serde_json::from_value(safe_graph).unwrap();
    ProductionGraphVerifier::new(registry())
        .verify(&safe_graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn k_guards_reject_labels_outside_every_closed_selector_domain() {
    for guard in [
        json!({"kind":"k_of_n","count":1,"values":[
            {"name":"verify","source":"signal","field":"verdict"},
            {"name":"verify","source":"error","field":null}
        ],"labels":["accepted","timeout","bogus"]}),
        json!({"kind":"k_of_map","count":1,
            "value":{"name":"mapVerify","source":"signal","field":"verdict"},
            "labels":["accepted","bogus"]}),
    ] {
        let graph = if guard["kind"] == "k_of_n" {
            let mut value = valid_graph();
            value["root"]["children"][2]["branches"][0]["when"] = guard;
            serde_json::from_value(value).unwrap()
        } else {
            map_choice_graph(1, guard)
        };
        let error = ProductionGraphVerifier::new(registry())
            .verify(&graph)
            .await
            .unwrap_err();
        assert!(rejection_codes(error).contains(&GraphDiagnosticCode::ChoiceExhaustiveness));
    }
}

#[tokio::test]
async fn single_map_item_cannot_have_success_and_error_outcomes() {
    let guard = json!({"kind":"all","guards":[
        {"kind":"k_of_map","count":1,
            "value":{"name":"mapVerify","source":"signal","field":"verdict"},"labels":["accepted"]},
        {"kind":"k_of_map","count":1,
            "value":{"name":"mapVerify","source":"error","field":null},"labels":["timeout"]}
    ]});
    let graph = map_choice_graph(1, guard);
    let error = ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap_err();
    assert!(rejection_codes(error).contains(&GraphDiagnosticCode::ChoiceExhaustiveness));
}

fn map_choice_graph(max_items: u64, guard: Value) -> GraphSpec {
    serde_json::from_value(json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":{"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}},
        "policy":{"policy":"policy.strict@1","default":"deny"},
        "root":{"kind":"seq","name":"root","state":{"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}},
        "children":[
            {"kind":"map","name":"map","state":{"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}},
             "body":{"kind":"verifier","name":"mapVerify","worker":"worker.verify@1",
                "input":{"kind":"null"},"output":{"kind":"record","fields":{}},
                "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
                "signals":{"verdict":["accepted","rejected"]},"diagnostic":{"kind":"record","fields":{}}},
             "over":{"source":"state","path":["items"]},"maxItems":max_items,"promotedStatePaths":[]},
            {"kind":"choice","name":"choice","state":{"kind":"record","fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}},
             "branches":[{"when":guard,"node":{"kind":"succeed","name":"selected","output":{"kind":"null"},"bindings":[]}}],
             "otherwise":{"kind":"fail","name":"failed","reason":"failed"},"promotedStatePaths":[]}
        ],"promotedStatePaths":[]}
    }))
    .unwrap()
}

#[tokio::test]
async fn every_parallel_join_and_group_control_domain_is_admitted() {
    let base_work = valid_graph()["root"]["children"][0].clone();
    for (join, field, label, verifier_branch) in [
        (json!({"kind":"any"}), "joined", "reached", false),
        (
            json!({"kind":"quorum","count":1}),
            "joined",
            "reached",
            false,
        ),
        (
            json!({
                "kind":"first",
                "when":{"kind":"in","value":{"name":"raceVerify","source":"signal","field":"verdict"},"labels":["accepted"]}
            }),
            "raced",
            "satisfied",
            true,
        ),
    ] {
        let mut left = base_work.clone();
        left["name"] = json!("left");
        left["writeBindings"][0]["value"]["node"] = json!("left");
        let mut right = base_work.clone();
        right["name"] = json!("right");
        right["writeBindings"][0]["value"]["node"] = json!("right");
        if verifier_branch {
            right = valid_graph()["root"]["children"][1].clone();
            right["name"] = json!("raceVerify");
        }
        let graph = graph_with_root_child(json!({
            "kind":"seq","name":"root","state":record(),"children":[
                {"kind":"par","name":"parallel","state":record(),"branches":[left,right],
                 "promotedStatePaths":[],"join":join},
                {"kind":"choice","name":"afterJoin","state":record(),"branches":[{
                    "when":{"kind":"in","value":{"name":"parallel","source":"group","field":field},"labels":[label]},
                    "node":{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
                 }],"otherwise":{"kind":"fail","name":"failed","reason":"failed"},
                 "promotedStatePaths":[]}
            ],"promotedStatePaths":[]
        }));
        ProductionGraphVerifier::new(registry())
            .verify(&graph)
            .await
            .unwrap();
    }
}

fn quorum_flow_graph(count: u64) -> GraphSpec {
    let flow_record = json!({
        "kind": "record",
        "fields": {
            "value": { "type": { "kind": "integer" }, "required": true },
            "leftResult": { "type": { "kind": "integer" }, "required": false },
            "rightResult": { "type": { "kind": "integer" }, "required": false }
        }
    });
    let branch = |name: &str, target: &str| {
        let writer = format!("{name}Writer");
        let routed = format!("{name}Routed");
        let failed = format!("{name}Failed");
        let continuation = format!("{name}Continuation");
        json!({
            "kind": "seq", "name": name, "state": flow_record,
            "children": [
                {
                    "kind": "step", "name": writer, "worker": "worker.main@1",
                    "input": record(),
                    "output": {"kind":"record","fields":{"result":{"type":{"kind":"integer"},"required":true}}},
                    "inputBindings": [{"target":["value"],"value":{"source":"state","path":["value"]}}],
                    "writeBindings": [{
                        "value":{"node":writer,"channel":"out","path":["result"]},
                        "target":[target]
                    }],
                    "timeoutMs": 1, "attempts": 1
                },
                {
                    "kind": "choice", "name": routed, "state": flow_record,
                    "branches": [{
                        "when": {
                            "kind":"in",
                            "value":{"name":writer,"source":"error","field":null},
                            "labels":["timeout","crash","malformed","refusal"]
                        },
                        "node":{"kind":"fail","name":failed,"reason":"worker_failed"}
                    }],
                    "otherwise": {
                        "kind": "step", "name": continuation, "worker": "worker.main@1",
                        "input": record(),
                        "output": {"kind":"record","fields":{"result":{"type":{"kind":"integer"},"required":true}}},
                        "inputBindings": [{"target":["value"],"value":{"source":"state","path":["value"]}}],
                        "writeBindings": [], "timeoutMs": 1, "attempts": 1
                    },
                    "promotedStatePaths": []
                }
            ],
            "promotedStatePaths": [[target]]
        })
    };

    serde_json::from_value(json!({
        "profile": "openengine.graph.full/v1",
        "initialInput": flow_record,
        "policy": { "policy": "policy.strict@1", "default": "deny" },
        "root": {
            "kind": "seq", "name": "root", "state": flow_record,
            "children": [
                {
                    "kind": "par", "name": "parallel", "state": flow_record,
                    "branches": [branch("left", "leftResult"), branch("right", "rightResult")],
                    "promotedStatePaths": [["leftResult"], ["rightResult"]],
                    "join": {"kind":"quorum","count":count}
                },
                {
                    "kind": "succeed", "name": "done",
                    "output": {
                        "kind": "record",
                        "fields": {
                            "leftResult": { "type": { "kind": "integer" }, "required": true },
                            "rightResult": { "type": { "kind": "integer" }, "required": true }
                        }
                    },
                    "bindings": [
                        {"target":["leftResult"],"value":{"source":"state","path":["leftResult"]}},
                        {"target":["rightResult"],"value":{"source":"state","path":["rightResult"]}}
                    ]
                }
            ],
            "promotedStatePaths": []
        }
    }))
    .unwrap()
}

#[tokio::test]
async fn quorum_promotion_and_flow_use_the_authored_completion_count() {
    let quorum_one = ProductionGraphVerifier::new(registry())
        .verify(&quorum_flow_graph(1))
        .await
        .unwrap_err();
    assert!(rejection_codes(quorum_one).contains(&GraphDiagnosticCode::UndefinedRead));

    ProductionGraphVerifier::new(registry())
        .verify(&quorum_flow_graph(2))
        .await
        .unwrap();
}

fn shared_quorum_flow_graph(count: u64) -> GraphSpec {
    let mut value = serde_json::to_value(quorum_flow_graph(2)).unwrap();
    let mut third = value["root"]["children"][0]["branches"][0].clone();
    third["name"] = json!("third");
    third["children"][0]["name"] = json!("thirdWriter");
    third["children"][0]["writeBindings"][0]["value"]["node"] = json!("thirdWriter");
    third["children"][1]["name"] = json!("thirdRouted");
    third["children"][1]["branches"][0]["when"]["value"]["name"] = json!("thirdWriter");
    third["children"][1]["branches"][0]["node"]["name"] = json!("thirdFailed");
    third["children"][1]["otherwise"]["name"] = json!("thirdContinuation");
    let left = value["root"]["children"][0]["branches"][0].clone();
    let right = value["root"]["children"][0]["branches"][1].clone();
    value["root"]["children"][0]["branches"] = json!([left, right, third]);
    value["root"]["children"][0]["join"]["count"] = json!(count);
    value["root"]["children"][0]["promotedStatePaths"] = json!([["leftResult"]]);
    value["root"]["children"][1]["output"]["fields"] = json!({
        "leftResult": { "type": { "kind": "integer" }, "required": true }
    });
    value["root"]["children"][1]["bindings"] = json!([{
        "target":["leftResult"],"value":{"source":"state","path":["leftResult"]}
    }]);
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn quorum_effects_cover_every_size_count_completion_set() {
    let quorum_one = ProductionGraphVerifier::new(registry())
        .verify(&shared_quorum_flow_graph(1))
        .await
        .unwrap_err();
    assert!(rejection_codes(quorum_one).contains(&GraphDiagnosticCode::UndefinedRead));

    ProductionGraphVerifier::new(registry())
        .verify(&shared_quorum_flow_graph(2))
        .await
        .unwrap();
}

fn parallel_terminal_graph(join: Value) -> GraphSpec {
    let work = valid_graph()["root"]["children"][0].clone();
    graph_with_root_child(json!({
        "kind":"seq", "name":"root", "state":record(), "children":[
            {
                "kind":"par", "name":"parallel", "state":record(),
                "branches":[
                    work,
                    {"kind":"succeed","name":"leftDone","output":{"kind":"null"},"bindings":[]},
                    {"kind":"succeed","name":"rightDone","output":{"kind":"null"},"bindings":[]}
                ],
                "promotedStatePaths":[], "join":join
            },
            {"kind":"succeed","name":"afterParallel","output":{"kind":"null"},"bindings":[]}
        ],
        "promotedStatePaths":[]
    }))
}

#[tokio::test]
async fn parallel_terminal_reachability_uses_the_join_completion_count() {
    ProductionGraphVerifier::new(registry())
        .verify(&parallel_terminal_graph(json!({"kind":"quorum","count":1})))
        .await
        .unwrap();

    for join in [json!({"kind":"quorum","count":2}), json!({"kind":"all"})] {
        let error = ProductionGraphVerifier::new(registry())
            .verify(&parallel_terminal_graph(join))
            .await
            .unwrap_err();
        let VerificationError::Rejected { diagnostics } = error else {
            panic!("unreachable successor must be a graph rejection")
        };
        assert!(diagnostics.iter().any(|diagnostic| {
            diagnostic.code == GraphDiagnosticCode::Reachability
                && diagnostic
                    .related_nodes
                    .iter()
                    .any(|name| name.as_str() == "afterParallel")
        }));
    }
}

#[tokio::test]
async fn quorum_completion_sets_preserve_shared_guard_correlations() {
    let branch = |name: &str, label: &str, worker: &str, terminal: &str| {
        json!({
            "kind":"choice", "name":name, "state":record(),
            "branches":[{
                "when":{
                    "kind":"in",
                    "value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":[label]
                },
                "node":{
                    "kind":"step", "name":worker, "worker":"worker.main@1",
                    "input":record(),
                    "output":{"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}},
                    "inputBindings":[{"target":["value"],"value":{"source":"state","path":["value"]}}],
                    "writeBindings":[], "timeoutMs":1, "attempts":1
                }
            }],
            "otherwise":{"kind":"succeed","name":terminal,"output":{"kind":"null"},"bindings":[]},
            "promotedStatePaths":[]
        })
    };
    let mut value = valid_graph();
    value["root"]["children"] = json!([
        value["root"]["children"][0].clone(),
        value["root"]["children"][1].clone(),
        {
            "kind":"par", "name":"correlatedQuorum", "state":record(),
            "branches":[
                branch("acceptedBranch", "accepted", "acceptedWork", "rejectedDone"),
                branch("rejectedBranch", "rejected", "rejectedWork", "acceptedDone")
            ],
            "promotedStatePaths":[],
            "join":{"kind":"quorum","count":2}
        }
    ]);
    let graph: GraphSpec = serde_json::from_value(value).unwrap();

    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn quorum_flow_uses_only_jointly_satisfiable_completion_sets() {
    let conditional = |name: &str, label: &str, worker: &str, terminal: &str| {
        json!({
            "kind":"choice", "name":name, "state":record(),
            "branches":[{
                "when":{
                    "kind":"in",
                    "value":{"name":"verify","source":"signal","field":"verdict"},
                    "labels":[label]
                },
                "node":{
                    "kind":"step", "name":worker, "worker":"worker.main@1",
                    "input":record(),
                    "output":{"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}},
                    "inputBindings":[{"target":["value"],"value":{"source":"state","path":["value"]}}],
                    "writeBindings":[], "timeoutMs":1, "attempts":1
                }
            }],
            "otherwise":{"kind":"succeed","name":terminal,"output":{"kind":"null"},"bindings":[]},
            "promotedStatePaths":[]
        })
    };
    let writer = json!({
        "kind":"seq", "name":"writerBranch", "state":record(),
        "children":[
            {
                "kind":"step", "name":"sharedWriter", "worker":"worker.main@1",
                "input":record(),
                "output":{"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}},
                "inputBindings":[{"target":["value"],"value":{"source":"state","path":["value"]}}],
                "writeBindings":[{
                    "value":{"node":"sharedWriter","channel":"out","path":["result"]},
                    "target":["result"]
                }],
                "timeoutMs":1, "attempts":1
            },
            {
                "kind":"choice", "name":"writerOutcome", "state":record(),
                "branches":[{
                    "when":{
                        "kind":"in", "value":{"name":"sharedWriter","source":"error"},
                        "labels":["timeout","crash","malformed","refusal"]
                    },
                    "node":{"kind":"succeed","name":"writerFailed","output":{"kind":"null"},"bindings":[]}
                }],
                "otherwise":{
                    "kind":"step", "name":"writerContinuation", "worker":"worker.main@1",
                    "input":record(),
                    "output":{"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}},
                    "inputBindings":[{"target":["value"],"value":{"source":"state","path":["value"]}}],
                    "writeBindings":[], "timeoutMs":1, "attempts":1
                },
                "promotedStatePaths":[]
            }
        ],
        "promotedStatePaths":[["result"]]
    });
    let mut value = valid_graph();
    value["root"]["children"] = json!([
        value["root"]["children"][0].clone(),
        value["root"]["children"][1].clone(),
        {
            "kind":"par", "name":"correlatedFlowQuorum", "state":record(),
            "branches":[
                conditional("acceptedBranch", "accepted", "acceptedWork", "rejectedDone"),
                writer,
                conditional("rejectedBranch", "rejected", "rejectedWork", "acceptedDone")
            ],
            "promotedStatePaths":[["result"]],
            "join":{"kind":"quorum","count":2}
        },
        {
            "kind":"succeed", "name":"done",
            "output":{"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}},
            "bindings":[{"target":["result"],"value":{"source":"state","path":["result"]}}]
        }
    ]);
    let graph: GraphSpec = serde_json::from_value(value).unwrap();

    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn output_signal_and_diagnostic_binding_channels_are_type_checked_and_admitted() {
    let graph = graph_with_root_child(json!({
        "kind":"seq","name":"root","state":record(),"children":[
            {"kind":"verifier","name":"verify","worker":"worker.verify@1",
             "input":{"kind":"null"},
             "output":{"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}},
             "inputBindings":[],
             "writeBindings":[
                {"value":{"node":"verify","channel":"out","path":["result"]},"target":["result"]},
                {"value":{"node":"verify","channel":"signal","path":["verdict"]},"target":["verdict"]},
                {"value":{"node":"verify","channel":"diagnostic","path":["code"]},"target":["diagnostic"]}
             ],
             "timeoutMs":1,"attempts":1,
             "signals":{"verdict":["accepted","rejected"]},
             "diagnostic":{"kind":"record","fields":{"code":{"type":{"kind":"number"},"required":true}}}},
            {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
        ],"promotedStatePaths":[]
    }));
    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}
