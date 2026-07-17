//! Deterministic production-verifier conformance envelopes.

use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use openengine_cluster_protocol::{GraphProfile, GraphSpec, WorkerDescriptor, WorkerRef};
use openengine_cluster_server::admission::{GraphVerifier, VerificationError, VerifiedGraph};
use openengine_cluster_server::graph_verifier::ProductionGraphVerifier;
use openengine_cluster_server::worker_registry::{WorkerRegistry, WorkerRegistryError};
use serde_json::{json, Value};

use crate::artifacts::Artifact;

const ROOT: &str = "protocol/openengine-cluster/v1/fixtures/verifier";

#[derive(Clone)]
pub struct VerifierFixtureRegistry {
    descriptors: BTreeMap<WorkerRef, WorkerDescriptor>,
    version_unavailable: BTreeSet<WorkerRef>,
}

#[async_trait]
impl WorkerRegistry for VerifierFixtureRegistry {
    async fn resolve(&self, worker: &WorkerRef) -> Result<WorkerDescriptor, WorkerRegistryError> {
        if self.version_unavailable.contains(worker) {
            return Err(WorkerRegistryError::VersionUnavailable {
                worker: worker.clone(),
            });
        }
        self.descriptors
            .get(worker)
            .cloned()
            .ok_or_else(|| WorkerRegistryError::NotFound {
                worker: worker.clone(),
            })
    }
}

#[must_use]
pub fn verifier_fixture_registry() -> VerifierFixtureRegistry {
    let mut descriptors = BTreeMap::new();
    insert_descriptor(
        &mut descriptors,
        "fixture.worker@1",
        descriptor_value("fixture.worker@1", null_type(), null_type(), None),
    );
    insert_descriptor(
        &mut descriptors,
        "fixture.data@1",
        descriptor_value(
            "fixture.data@1",
            data_input_type(),
            result_integer_type(),
            None,
        ),
    );
    insert_descriptor(
        &mut descriptors,
        "fixture.verifier@1",
        descriptor_value(
            "fixture.verifier@1",
            null_type(),
            result_integer_type(),
            Some(verifier_contract()),
        ),
    );
    insert_registry_fault_descriptors(&mut descriptors);
    VerifierFixtureRegistry {
        descriptors,
        version_unavailable: BTreeSet::from([worker_ref("fixture.version-unavailable@2")]),
    }
}

fn insert_descriptor(
    descriptors: &mut BTreeMap<WorkerRef, WorkerDescriptor>,
    requested: &str,
    value: Value,
) {
    descriptors.insert(
        worker_ref(requested),
        serde_json::from_value(value).expect("fixture descriptor is valid"),
    );
}

fn insert_registry_fault_descriptors(descriptors: &mut BTreeMap<WorkerRef, WorkerDescriptor>) {
    let mut invalid: WorkerDescriptor = serde_json::from_value(descriptor_value(
        "fixture.invalid-contract@1",
        null_type(),
        null_type(),
        None,
    ))
    .unwrap();
    invalid.contract.errors.pop();
    descriptors.insert(worker_ref("fixture.invalid-contract@1"), invalid);

    insert_descriptor(
        descriptors,
        "fixture.identity@1",
        descriptor_value("fixture.returned-other@1", null_type(), null_type(), None),
    );
    let mut profile: WorkerDescriptor = serde_json::from_value(descriptor_value(
        "fixture.profile@1",
        null_type(),
        null_type(),
        None,
    ))
    .unwrap();
    profile.graph_profiles = vec![GraphProfile::SingleWorker];
    descriptors.insert(worker_ref("fixture.profile@1"), profile);
}

fn descriptor_value(worker: &str, input: Value, output: Value, verifier: Option<Value>) -> Value {
    json!({
        "worker": worker,
        "graphProfiles": ["openengine.graph.full/v1"],
        "binding": { "protocol": "acp", "version": "1", "profile": "openengine.worker.acp/v1" },
        "contract": {
            "input": input,
            "output": output,
            "verifier": verifier,
            "errors": ["timeout", "crash", "malformed", "refusal"]
        },
        "capabilityPolicy": { "autonomy": "strict", "permissionPolicy": "policy.strict@1" },
        "artifactProfile": {
            "allowedTypeIds": ["openengine.result@1"],
            "allowedMediaTypes": ["application/json"],
            "minimumRedaction": "internal"
        },
        "credentialRequirements": []
    })
}

fn verifier_contract() -> Value {
    json!({
        "signals": { "verdict": ["accepted", "rejected"] },
        "diagnostic": diagnostic_integer_type()
    })
}

pub async fn verify_fixture_graph(graph: &GraphSpec) -> Result<VerifiedGraph, VerificationError> {
    ProductionGraphVerifier::new(verifier_fixture_registry())
        .verify(graph)
        .await
}

pub async fn graph_verifier_fixture_artifacts() -> Vec<Artifact> {
    let mut artifacts = Vec::new();
    for (name, graph_value) in positive_cases().into_iter().chain(negative_cases()) {
        let graph: GraphSpec = serde_json::from_value(graph_value.clone())
            .expect("verifier fixture graph must satisfy the wire contract");
        let expected = result_value(verify_fixture_graph(&graph).await);
        artifacts.push(json_artifact(
            format!("{ROOT}/{name}"),
            json!({ "graph": graph_value, "expected": expected }),
        ));
    }
    artifacts
}

#[must_use]
pub fn result_value(result: Result<VerifiedGraph, VerificationError>) -> Value {
    match result {
        Ok(verified) => json!({
            "status": "verified",
            "compiledIr": verified.compiled_ir,
            "diagnostics": verified.diagnostics
        }),
        Err(VerificationError::Rejected { diagnostics }) => {
            json!({ "status": "rejected", "diagnostics": diagnostics })
        }
        Err(VerificationError::Internal(message)) => {
            panic!("fixture verification reached internal failure: {message}")
        }
    }
}

fn positive_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("positive/basic.json", basic_graph()),
        ("positive/binding-channels.json", binding_channels_graph()),
        ("positive/guard-in.json", guarded_graph(in_guard())),
        (
            "positive/guard-all.json",
            guarded_graph(json!({"kind":"all","guards":[in_guard()]})),
        ),
        (
            "positive/guard-any.json",
            guarded_graph(json!({"kind":"any","guards":[in_guard(), error_guard()]})),
        ),
        (
            "positive/guard-not.json",
            guarded_graph(json!({"kind":"not","guard":in_guard()})),
        ),
        ("positive/guard-k-of-n.json", guarded_graph(k_of_n_guard())),
        ("positive/map-item-k-of-map.json", map_item_graph()),
        ("positive/map-signal-and-group.json", map_signal_graph()),
        ("positive/loop-and-group.json", loop_graph()),
        ("positive/join-all.json", parallel_graph("all")),
        ("positive/join-any.json", parallel_graph("any")),
        ("positive/join-quorum.json", parallel_graph("quorum")),
        ("positive/join-first.json", parallel_graph("first")),
        ("positive/nested-structural-folds.json", nested_fold_graph()),
        (
            "positive/exhaustive-terminal-choice.json",
            exhaustive_terminal_choice_graph(),
        ),
        (
            "positive/success-routed-write.json",
            success_routed_write_graph(),
        ),
    ]
}

fn negative_cases() -> Vec<(&'static str, Value)> {
    let mut cases = vec![
        ("negative/duplicate-node.json", duplicate_node_graph()),
        ("negative/terminal-fallthrough.json", fallthrough_graph()),
        (
            "negative/illegal-control-selector.json",
            illegal_control_graph(),
        ),
        ("negative/undefined-read.json", undefined_read_graph()),
        (
            "negative/output-write-error-path.json",
            output_write_error_path_graph(),
        ),
        ("negative/cyclic-read.json", cyclic_read_graph()),
        ("negative/type-mismatch.json", type_mismatch_graph()),
        ("negative/dead-choice.json", dead_choice_graph()),
        ("negative/dead-otherwise.json", dead_otherwise_graph()),
        (
            "negative/non-exhaustive-choice.json",
            non_exhaustive_choice_graph(),
        ),
        (
            "negative/unsatisfiable-loop.json",
            unsatisfiable_loop_graph(),
        ),
        ("negative/invalid-quorum.json", invalid_quorum_graph()),
        (
            "negative/parallel-write-conflict.json",
            write_conflict_graph(),
        ),
        ("negative/unsafe-promotion.json", unsafe_promotion_graph()),
        (
            "negative/impossible-map-outcomes.json",
            impossible_map_outcomes_graph(),
        ),
        ("negative/closed-k-labels.json", closed_k_labels_graph()),
    ];
    cases.extend(registry_negative_cases());
    cases
}

fn registry_negative_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "negative/registry-not-found.json",
            worker_graph("fixture.missing@1"),
        ),
        (
            "negative/registry-version-unavailable.json",
            worker_graph("fixture.version-unavailable@2"),
        ),
        (
            "negative/registry-descriptor-contract.json",
            worker_graph("fixture.invalid-contract@1"),
        ),
        (
            "negative/registry-descriptor-identity.json",
            worker_graph("fixture.identity@1"),
        ),
        (
            "negative/registry-graph-profile.json",
            worker_graph("fixture.profile@1"),
        ),
        ("negative/registry-input.json", registry_input_graph()),
        ("negative/registry-output.json", registry_output_graph()),
        (
            "negative/registry-verifier-contract.json",
            registry_verifier_contract_graph(),
        ),
        (
            "negative/registry-signal-field.json",
            registry_signal_field_graph(),
        ),
        (
            "negative/registry-signal-labels.json",
            registry_signal_labels_graph(),
        ),
        (
            "negative/registry-diagnostic.json",
            registry_diagnostic_graph(),
        ),
    ]
}

fn graph(initial_input: Value, state: Value, children: Vec<Value>) -> Value {
    json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":initial_input,
        "policy":{"policy":"policy.strict@1","default":"deny"},
        "root":{
            "kind":"seq","name":"root","state":state,
            "children":children,
            "promotedStatePaths":[]
        }
    })
}

fn null_step(name: &str, worker: &str) -> Value {
    json!({
        "kind":"step","name":name,"worker":worker,
        "input":null_type(),"output":null_type(),
        "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1
    })
}

fn data_step(name: &str) -> Value {
    json!({
        "kind":"step","name":name,"worker":"fixture.data@1",
        "input":data_input_type(),"output":result_number_type(),
        "inputBindings":[{"target":["value"],"value":{"source":"state","path":["value"]}}],
        "writeBindings":[{"value":{"node":name,"channel":"out","path":["result"]},"target":["result"]}],
        "timeoutMs":1,"attempts":2
    })
}

fn verifier_node(name: &str) -> Value {
    json!({
        "kind":"verifier","name":name,"worker":"fixture.verifier@1",
        "input":null_type(),"output":result_number_type(),
        "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1,
        "signals":{"verdict":["accepted","rejected"]},
        "diagnostic":diagnostic_number_type()
    })
}

fn succeed(name: &str) -> Value {
    json!({"kind":"succeed","name":name,"output":null_type(),"bindings":[]})
}

fn fail(name: &str) -> Value {
    json!({"kind":"fail","name":name,"reason":"rejected"})
}

fn choice(name: &str, state: Value, guard: Value, selected: Value) -> Value {
    json!({
        "kind":"choice","name":name,"state":state,
        "branches":[{"when":guard,"node":selected}],
        "otherwise":fail(&format!("{name}Otherwise")),
        "promotedStatePaths":[]
    })
}

fn in_guard() -> Value {
    json!({
        "kind":"in",
        "value":{"name":"verify","source":"signal","field":"verdict"},
        "labels":["accepted"]
    })
}

fn error_guard() -> Value {
    json!({
        "kind":"in",
        "value":{"name":"verify","source":"error","field":null},
        "labels":["timeout"]
    })
}

fn k_of_n_guard() -> Value {
    json!({
        "kind":"k_of_n","count":1,
        "values":[
            {"name":"verify","source":"signal","field":"verdict"},
            {"name":"verify","source":"error","field":null}
        ],
        "labels":["accepted","timeout"]
    })
}

fn basic_graph() -> Value {
    graph(
        null_type(),
        null_type(),
        vec![null_step("work", "fixture.worker@1"), succeed("done")],
    )
}

fn binding_channels_graph() -> Value {
    let mut verifier = verifier_node("verify");
    verifier["writeBindings"] = json!([
        {"value":{"node":"verify","channel":"out","path":["result"]},"target":["result"]},
        {"value":{"node":"verify","channel":"signal","path":["verdict"]},"target":["verdict"]},
        {"value":{"node":"verify","channel":"diagnostic","path":["code"]},"target":["diagnostic"]}
    ]);
    graph(
        data_state_type(),
        data_state_type(),
        vec![verifier, succeed("done")],
    )
}

fn state_result_succeed(name: &str) -> Value {
    json!({
        "kind":"succeed","name":name,"output":result_number_type(),
        "bindings":[{"target":["result"],"value":{"source":"state","path":["result"]}}]
    })
}

fn success_routed_write_graph() -> Value {
    let route = json!({
        "kind":"choice","name":"routeWork","state":data_state_type(),
        "branches":[{
            "when":{"kind":"in","value":{"name":"produce","source":"error","field":null},
                "labels":["timeout","crash","malformed","refusal"]},
            "node":fail("workFailed")
        }],
        "otherwise":state_result_succeed("done"),"promotedStatePaths":[]
    });
    graph(
        data_state_type(),
        data_state_type(),
        vec![data_step("produce"), route],
    )
}

fn guarded_graph(guard: Value) -> Value {
    graph(
        data_state_type(),
        data_state_type(),
        vec![
            verifier_node("verify"),
            choice("decision", data_state_type(), guard, succeed("selected")),
        ],
    )
}

fn map_item_graph() -> Value {
    let body = json!({
        "kind":"step","name":"mapWork","worker":"fixture.data@1",
        "input":data_input_type(),"output":result_number_type(),
        "inputBindings":[{"target":["value"],"value":{"source":"item","path":["value"]}}],
        "writeBindings":[],"timeoutMs":1,"attempts":1
    });
    let map = json!({
        "kind":"map","name":"map","state":data_state_type(),"body":body,
        "over":{"source":"state","path":["items"]},"maxItems":2,"promotedStatePaths":[]
    });
    let guard = json!({
        "kind":"k_of_map","count":1,
        "value":{"name":"mapWork","source":"error","field":null},"labels":["timeout"]
    });
    graph(
        data_state_type(),
        data_state_type(),
        vec![
            map,
            choice("afterMap", data_state_type(), guard, succeed("selected")),
        ],
    )
}

fn map_signal_graph() -> Value {
    let map = json!({
        "kind":"map","name":"map","state":data_state_type(),"body":verifier_node("mapVerify"),
        "over":{"source":"state","path":["items"]},"maxItems":2,"promotedStatePaths":[]
    });
    let aggregate = json!({
        "kind":"k_of_map","count":1,
        "value":{"name":"mapVerify","source":"signal","field":"verdict"},"labels":["accepted"]
    });
    let overflow = json!({
        "kind":"in","value":{"name":"map","source":"group","field":"overflow"},
        "labels":["overflow"]
    });
    let guard = json!({"kind":"any","guards":[aggregate,overflow]});
    let after_group = choice("mapControl", data_state_type(), guard, succeed("done"));
    graph(data_state_type(), data_state_type(), vec![map, after_group])
}

fn loop_graph() -> Value {
    let loop_node = json!({
        "kind":"loop","name":"repeat","state":data_state_type(),"body":verifier_node("verify"),
        "until":in_guard(),"maxIterations":3,"promotedStatePaths":[]
    });
    let terminated = json!({
        "kind":"in","value":{"name":"repeat","source":"group","field":"terminated"},
        "labels":["converged"]
    });
    graph(
        data_state_type(),
        data_state_type(),
        vec![
            loop_node,
            choice(
                "loopControl",
                data_state_type(),
                terminated,
                succeed("done"),
            ),
        ],
    )
}

fn parallel_graph(join_kind: &str) -> Value {
    let (branches, join, field) = match join_kind {
        "all" => (
            vec![
                null_step("left", "fixture.worker@1"),
                null_step("right", "fixture.worker@1"),
            ],
            json!({"kind":"all"}),
            "joined",
        ),
        "any" => (
            vec![
                null_step("left", "fixture.worker@1"),
                null_step("right", "fixture.worker@1"),
            ],
            json!({"kind":"any"}),
            "joined",
        ),
        "quorum" => (
            vec![
                null_step("left", "fixture.worker@1"),
                null_step("right", "fixture.worker@1"),
            ],
            json!({"kind":"quorum","count":1}),
            "joined",
        ),
        "first" => (
            vec![
                verifier_node("raceVerify"),
                null_step("right", "fixture.worker@1"),
            ],
            json!({"kind":"first","when":{"kind":"in",
                "value":{"name":"raceVerify","source":"signal","field":"verdict"},"labels":["accepted"]}}),
            "raced",
        ),
        _ => unreachable!("join domain is closed"),
    };
    let par = json!({
        "kind":"par","name":"parallel","state":data_state_type(),
        "branches":branches,"promotedStatePaths":[],"join":join
    });
    let labels = if join_kind == "first" {
        json!(["satisfied"])
    } else {
        json!(["reached"])
    };
    let control = json!({
        "kind":"in","value":{"name":"parallel","source":"group","field":field},"labels":labels
    });
    graph(
        data_state_type(),
        data_state_type(),
        vec![
            par,
            choice("joinControl", data_state_type(), control, succeed("done")),
        ],
    )
}

fn nested_fold_graph() -> Value {
    let loop_node = json!({
        "kind":"loop","name":"innerLoop","state":data_state_type(),"body":verifier_node("verify"),
        "until":in_guard(),"maxIterations":2,"promotedStatePaths":[]
    });
    let map = json!({
        "kind":"map","name":"outerMap","state":data_state_type(),"body":loop_node,
        "over":{"source":"state","path":["items"]},"maxItems":3,"promotedStatePaths":[]
    });
    let par = json!({
        "kind":"par","name":"parallel","state":data_state_type(),
        "branches":[map, null_step("peer", "fixture.worker@1")],
        "promotedStatePaths":[],"join":{"kind":"all"}
    });
    graph(
        data_state_type(),
        data_state_type(),
        vec![par, succeed("done")],
    )
}

fn exhaustive_terminal_choice_graph() -> Value {
    let decision = json!({
        "kind":"choice","name":"decision","state":data_state_type(),
        "branches":[
            {"when":{"kind":"in","value":{"name":"verify","source":"signal","field":"verdict"},
                "labels":["accepted","rejected"]},"node":succeed("done")},
            {"when":{"kind":"in","value":{"name":"verify","source":"error","field":null},
                "labels":["timeout","crash","malformed","refusal"]},"node":fail("failed")}
        ],
        "otherwise":null,"promotedStatePaths":[]
    });
    graph(
        data_state_type(),
        data_state_type(),
        vec![verifier_node("verify"), decision],
    )
}

fn duplicate_node_graph() -> Value {
    graph(
        null_type(),
        null_type(),
        vec![null_step("work", "fixture.worker@1"), succeed("work")],
    )
}

fn fallthrough_graph() -> Value {
    json!({
        "profile":"openengine.graph.full/v1","initialInput":null_type(),
        "policy":{"policy":"policy.strict@1","default":"deny"},
        "root":null_step("work", "fixture.worker@1")
    })
}

fn illegal_control_graph() -> Value {
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

fn undefined_read_graph() -> Value {
    let mut step = data_step("work");
    step["inputBindings"][0]["value"]["path"] = json!(["result"]);
    graph(
        data_state_type(),
        data_state_type(),
        vec![step, succeed("done")],
    )
}

fn output_write_error_path_graph() -> Value {
    graph(
        data_state_type(),
        data_state_type(),
        vec![data_step("produce"), state_result_succeed("done")],
    )
}

fn cyclic_read_graph() -> Value {
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

fn type_mismatch_graph() -> Value {
    let mut step = data_step("work");
    step["input"]["fields"]["value"]["type"] = json!({"kind":"string"});
    graph(
        data_state_type(),
        data_state_type(),
        vec![step, succeed("done")],
    )
}

fn dead_choice_graph() -> Value {
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

fn dead_otherwise_graph() -> Value {
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

fn non_exhaustive_choice_graph() -> Value {
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

fn unsatisfiable_loop_graph() -> Value {
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

fn invalid_quorum_graph() -> Value {
    let par = json!({
        "kind":"par","name":"parallel","state":null_type(),
        "branches":[null_step("left", "fixture.worker@1"),null_step("right", "fixture.worker@1")],
        "promotedStatePaths":[],"join":{"kind":"quorum","count":3}
    });
    graph(null_type(), null_type(), vec![par, succeed("done")])
}

fn write_conflict_graph() -> Value {
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

fn unsafe_promotion_graph() -> Value {
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

fn impossible_map_outcomes_graph() -> Value {
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

fn closed_k_labels_graph() -> Value {
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

fn worker_graph(worker: &str) -> Value {
    graph(
        null_type(),
        null_type(),
        vec![null_step("work", worker), succeed("done")],
    )
}

fn registry_input_graph() -> Value {
    let mut step = null_step("work", "fixture.worker@1");
    step["input"] = json!({"kind":"number"});
    graph(null_type(), null_type(), vec![step, succeed("done")])
}

fn registry_output_graph() -> Value {
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

fn registry_verifier_contract_graph() -> Value {
    let step = json!({
        "kind":"step","name":"work","worker":"fixture.verifier@1",
        "input":null_type(),"output":result_number_type(),
        "inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1
    });
    graph(null_type(), null_type(), vec![step, succeed("done")])
}

fn registry_signal_field_graph() -> Value {
    let mut verifier = verifier_node("verify");
    verifier["signals"] = json!({"other":["accepted"]});
    graph(null_type(), null_type(), vec![verifier, succeed("done")])
}

fn registry_signal_labels_graph() -> Value {
    let mut verifier = verifier_node("verify");
    verifier["signals"] = json!({"verdict":["accepted"]});
    graph(null_type(), null_type(), vec![verifier, succeed("done")])
}

fn registry_diagnostic_graph() -> Value {
    let mut verifier = verifier_node("verify");
    verifier["diagnostic"] = json!({"kind":"string"});
    graph(null_type(), null_type(), vec![verifier, succeed("done")])
}

fn null_type() -> Value {
    json!({"kind":"null"})
}

fn data_input_type() -> Value {
    json!({"kind":"record","fields":{"value":{"type":{"kind":"integer"},"required":true}}})
}

fn result_integer_type() -> Value {
    json!({"kind":"record","fields":{"result":{"type":{"kind":"integer"},"required":true}}})
}

fn result_number_type() -> Value {
    json!({"kind":"record","fields":{"result":{"type":{"kind":"number"},"required":true}}})
}

fn diagnostic_integer_type() -> Value {
    json!({"kind":"record","fields":{"code":{"type":{"kind":"integer"},"required":true}}})
}

fn diagnostic_number_type() -> Value {
    json!({"kind":"record","fields":{"code":{"type":{"kind":"number"},"required":true}}})
}

fn data_state_type() -> Value {
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

fn worker_ref(value: &str) -> WorkerRef {
    WorkerRef::new(value).expect("fixture worker reference is valid")
}

fn json_artifact(relative_path: String, value: Value) -> Artifact {
    let mut bytes = serde_json::to_vec_pretty(&value).expect("fixture serialization must succeed");
    bytes.push(b'\n');
    Artifact {
        relative_path,
        bytes,
    }
}
