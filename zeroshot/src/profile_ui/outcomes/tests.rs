use super::*;
use async_trait::async_trait;
use openengine_cluster_protocol::{WorkerDescriptor, WorkerErrorCode, WorkerOutcome, WorkerRef};
use openengine_cluster_server::admission::{GraphVerifier, VerifiedGraph};
use openengine_cluster_server::graph_verifier::ProductionGraphVerifier;
use openengine_cluster_server::worker_registry::{WorkerRegistry, WorkerRegistryError};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;
use crate::full_v1_reducer::{
    Decision, DurableExecution, DurableExecutionState, ExecutionId, FullV1Reducer, HistoryPosition,
    NodeInstanceId, Reduction, ReductionInput, StructuralOccurrence, TerminalProjection,
};

fn worker(name: &str) -> Value {
    json!({
        "kind":"step", "name":name, "worker":format!("{name}@1"),
        "instructions":"Write a useful file.", "input":{"kind":"null"},
        "output":{"kind":"null"}, "inputBindings":[], "writeBindings":[], "attempts":1
    })
}
fn done() -> Value {
    json!({"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]})
}
fn sequence(name: &str, children: Vec<Value>) -> Value {
    json!({"kind":"seq","name":name,"state":{"kind":"record","fields":{}},"children":children,"promotedStatePaths":[]})
}
fn graph(children: Vec<Value>) -> Value {
    json!({
        "profile":"openengine.graph.full/v1",
        "initialInput":{"kind":"record","fields":{}},
        "policy":{"policy":"policy.native-v2@1","default":"deny"},
        "root":sequence("run",children)
    })
}
fn transform(graph: Value, action: Value) -> Result<Value, String> {
    let request = serde_json::from_value(
        json!({"graph":graph,"runtime":{"harness":"","nodes":{}},"action":action}),
    )
    .map_err(|error| error.to_string())?;
    let document = apply(request).map_err(|error| error.message)?;
    serde_json::to_value(document).map_err(|error| error.to_string())
}
fn parallel(branches: Vec<Value>) -> Value {
    json!({
        "kind":"par", "name":"plans", "state":{"kind":"record","fields":{}},
        "branches":branches, "promotedStatePaths":[], "join":{"kind":"all"}
    })
}

fn mapped(body: Value) -> Value {
    json!({
        "kind":"map", "name":"items", "state":{"kind":"record","fields":{}},
        "body":body, "over":{"source":"state","path":["items"]},
        "maxItems":8, "promotedStatePaths":[]
    })
}

fn protected_graph(source: Value, node: &str) -> Value {
    transform(source, json!({"kind":"protect","node":node})).assert_value()["graph"].clone()
}

fn repeat(body: Value) -> Value {
    json!({
        "kind":"loop", "name":"repeat", "state":{"kind":"record","fields":{}},
        "body":body, "maxIterations":3, "promotedStatePaths":[]
    })
}

#[test]
fn loop_protection_refreshes_error_exits_without_duplicate_guards_or_exhaustion_failure() {
    let source = graph(vec![repeat(worker("write")), worker("publish"), done()]);
    let action = json!({"kind":"protect","node":"repeat"});
    let first = transform(source.clone(), action.clone()).assert_value()["graph"].clone();
    let until = &first["root"]["children"][0]["until"];
    assert_eq!(until["value"]["name"], "write");
    assert_eq!(
        first["root"]["children"][1]["branches"]
            .as_array()
            .assert_value()
            .len(),
        1
    );
    assert_eq!(first["root"]["children"][1]["branches"][0]["when"], *until);
    assert_eq!(
        first["root"]["children"][1]["otherwise"]["children"],
        json!([source["root"]["children"][1], source["root"]["children"][2]])
    );
    assert_eq!(
        transform(first.clone(), action.clone()).assert_value()["graph"],
        first
    );

    let mut expanded = first;
    expanded["root"]["children"][0]["body"] =
        sequence("round", vec![worker("write"), verifier("review")]);
    let expanded = transform(expanded, action.clone()).assert_value()["graph"].clone();
    let guards = expanded["root"]["children"][0]["until"]["guards"]
        .as_array()
        .assert_value();
    assert_eq!(guards.len(), 2);
    assert_eq!(guards[0]["value"]["name"], "write");
    assert_eq!(guards[1]["value"]["name"], "review");
    assert_eq!(
        expanded["root"]["children"][1]["branches"][0]["when"],
        expanded["root"]["children"][0]["until"]
    );
    assert_eq!(
        transform(expanded.clone(), action).assert_value()["graph"],
        expanded
    );
}

#[test]
fn loop_protection_keeps_authored_verdict_condition_and_requires_a_guaranteed_body() {
    let condition = json!({
        "kind":"in",
        "value":{"name":"review","source":"signal","field":"verdict"},
        "labels":["accepted"]
    });
    let mut repeated = repeat(sequence("round", vec![worker("write"), verifier("review")]));
    repeated["until"] = condition.clone();
    let action = json!({"kind":"protect","node":"repeat"});
    let result =
        transform(graph(vec![repeated, done()]), action.clone()).assert_value()["graph"].clone();
    let guards = result["root"]["children"][0]["until"]["guards"]
        .as_array()
        .assert_value();
    assert_eq!(guards.len(), 3);
    assert_eq!(guards[0], condition);
    assert_eq!(
        transform(result.clone(), action.clone()).assert_value()["graph"],
        result
    );
    for body in [
        done(),
        repeat(worker("write")),
        json!({
            "kind":"choice", "name":"route", "state":{"kind":"record","fields":{}},
            "branches":[{"when":condition,"node":worker("write")}],
            "otherwise":worker("other"), "promotedStatePaths":[]
        }),
        {
            let mut group = parallel(vec![verifier("left"), verifier("right")]);
            group["join"] = json!({"kind":"any"});
            group
        },
    ] {
        assert!(transform(graph(vec![repeat(body), done()]), action.clone()).is_err());
    }
    assert!(transform(graph(vec![repeat(worker("write"))]), action).is_err());
}

#[test]
fn completion_reserves_dangling_references_and_incomplete_runtime_names() {
    let mut source = graph(vec![worker("write")]);
    source["root"]["children"][0]["writeBindings"] = json!([{
        "target":["missing"],
        "value":{"node":"completed","channel":"out","path":["missing"]}
    }]);
    let runtime =
        json!({"harness":"","provider":"","nodes":{"completed_1":{"kind":"agent","model":""}}});
    let request = serde_json::from_value(
        json!({"graph":source,"runtime":runtime,"action":{"kind":"complete","owner":"run"}}),
    )
    .assert_value();
    let result = serde_json::to_value(apply(request).map_err(|error| error.message).assert_value())
        .assert_value();
    assert_eq!(result["runtime"], runtime);
    assert_eq!(
        result["graph"]["root"]["children"][1]["name"],
        "completed_2"
    );
    assert_eq!(
        result["graph"]["root"]["children"][0],
        source["root"]["children"][0]
    );
}

#[test]
fn completion_rejects_existing_terminal_and_non_sequence_and_ambiguous_names() {
    for (source, owner) in [
        (graph(vec![worker("write"), done()]), "run"),
        (graph(vec![worker("write")]), "write"),
        (graph(vec![worker("write"), worker("write")]), "write"),
        (
            graph(vec![parallel(vec![sequence(
                "branch",
                vec![worker("write")],
            )])]),
            "branch",
        ),
    ] {
        assert!(transform(source, json!({"kind":"complete","owner":owner})).is_err());
    }
}

#[test]
fn failure_reason_changes_only_exact_failure_and_native_type_rejects_reserved_reasons() {
    let source = graph(vec![
        json!({"kind":"fail","name":"failed","reason":"old_reason"}),
    ]);
    let result = transform(
        source.clone(),
        json!({"kind":"failure_reason","terminal":"failed","reason":"budget_exhausted"}),
    )
    .assert_value();
    let mut expected = source.clone();
    expected["root"]["children"][0]["reason"] = json!("budget_exhausted");
    assert_eq!(result["graph"], expected);
    for reason in [
        "unhandled",
        "runtime_failed",
        "runtime_lost",
        "invalid reason",
    ] {
        assert!(
            transform(
                source.clone(),
                json!({"kind":"failure_reason","terminal":"failed","reason":reason})
            )
            .is_err()
        );
    }
    assert!(
        transform(
            graph(vec![worker("write")]),
            json!({"kind":"failure_reason","terminal":"write","reason":"failed"})
        )
        .is_err()
    );
}

#[test]
fn serial_protection_preserves_entire_continuation_and_is_idempotent() {
    let source = graph(vec![worker("write"), worker("review"), done()]);
    let action = json!({"kind":"protect","node":"write"});
    let result = transform(source.clone(), action.clone()).assert_value();
    let root = &result["graph"]["root"];
    assert_eq!(root["children"][0], source["root"]["children"][0]);
    let route = &root["children"][1];
    assert_eq!(route["branches"][0]["when"]["value"]["name"], "write");
    assert_eq!(
        route["branches"][0]["when"]["labels"]
            .as_array()
            .assert_value()
            .len(),
        4
    );
    assert_eq!(
        route["otherwise"]["children"],
        json!([source["root"]["children"][1], source["root"]["children"][2]])
    );
    assert_eq!(
        transform(result["graph"].clone(), action).assert_value()["graph"],
        result["graph"]
    );
}

#[test]
fn parallel_failure_is_after_join_and_refresh_includes_added_writer() {
    let original = transform(
        graph(vec![worker("venue"), done()]),
        json!({"kind":"protect","node":"venue"}),
    )
    .assert_value();
    let mut expanded = original["graph"].clone();
    let group = parallel(vec![
        expanded["root"]["children"][0].clone(),
        worker("agenda"),
    ]);
    expanded["root"]["children"][0] = group.clone();
    let result =
        transform(expanded.clone(), json!({"kind":"protect","node":"plans"})).assert_value();
    let root = &result["graph"]["root"];
    assert_eq!(root["children"][0], group);
    assert_eq!(root["children"].as_array().assert_value().len(), 2);
    let mut expected_route = expanded["root"]["children"][1].clone();
    expected_route["branches"][0]["when"] = root["children"][1]["branches"][0]["when"].clone();
    assert_eq!(root["children"][1], expected_route);
    let selectors = root["children"][1]["branches"][0]["when"]["guards"]
        .as_array()
        .assert_value();
    assert_eq!(selectors.len(), 2);
    assert_eq!(selectors[0]["value"]["name"], "venue");
    assert_eq!(selectors[1]["value"]["name"], "agenda");
}

#[test]
fn map_failure_uses_aggregate_errors_and_overflow_after_collection() {
    let mapped = mapped(worker("draft"));
    let result = transform(
        graph(vec![mapped.clone(), done()]),
        json!({"kind":"protect","node":"items"}),
    )
    .assert_value();
    let root = &result["graph"]["root"];
    assert_eq!(root["children"][0], mapped);
    let guards = root["children"][1]["branches"][0]["when"]["guards"]
        .as_array()
        .assert_value();
    assert_eq!(guards[0]["value"]["field"], "overflow");
    assert_eq!(guards[1]["kind"], "k_of_map");
    assert_eq!(guards[1]["count"], 1);
    assert_eq!(guards[1]["value"]["name"], "draft");
}

#[test]
fn protection_rejects_branch_terminals_other_joins_and_partial_custom_guards() {
    let mut group = parallel(vec![sequence(
        "branch",
        vec![worker("write"), worker("review")],
    )]);
    assert!(
        transform(
            graph(vec![group.clone(), done()]),
            json!({"kind":"protect","node":"write"})
        )
        .is_err()
    );
    group["join"] = json!({"kind":"any"});
    assert!(
        transform(
            graph(vec![group, done()]),
            json!({"kind":"protect","node":"plans"})
        )
        .is_err()
    );
    assert!(
        transform(
            graph(vec![worker("write")]),
            json!({"kind":"protect","node":"write"})
        )
        .is_err()
    );
    let mut protected = protected_graph(graph(vec![worker("write"), done()]), "write");
    protected["root"]["children"][1]["branches"][0]["when"]["labels"] = json!(["crash"]);
    assert!(transform(protected, json!({"kind":"protect","node":"write"})).is_err());
}

#[test]
fn map_policy_refresh_preserves_overflow_and_adds_new_mapped_workers() {
    let mut protected = protected_graph(graph(vec![mapped(worker("draft")), done()]), "items");
    protected["root"]["children"][0]["body"] =
        sequence("map_steps", vec![worker("draft"), worker("review")]);

    let refreshed = transform(protected, json!({"kind":"protect","node":"items"})).assert_value();
    let guards = refreshed["graph"]["root"]["children"][1]["branches"][0]["when"]["guards"]
        .as_array()
        .assert_value();
    assert_eq!(guards.len(), 3);
    assert_eq!(guards[0]["value"]["field"], "overflow");
    assert_eq!(guards[1]["value"]["name"], "draft");
    assert_eq!(guards[2]["value"]["name"], "review");
}

#[test]
fn policy_refresh_refuses_to_overwrite_custom_continuation_handling() {
    let mut protected = protected_graph(graph(vec![worker("write"), done()]), "write");
    protected["root"]["children"][0] = parallel(vec![worker("write"), worker("review")]);
    let custom_guard = json!({
        "kind":"in", "value":{"name":"review","source":"error","field":null},
        "labels":["crash"]
    });
    protected["root"]["children"][1]["otherwise"] = json!({
        "kind":"choice", "name":"custom_review", "state":{"kind":"record","fields":{}},
        "branches":[{
            "when":custom_guard,
            "node":{"kind":"fail","name":"custom_failed","reason":"review_failed"}
        }],
        "otherwise":done(), "promotedStatePaths":[]
    });

    let error = transform(protected, json!({"kind":"protect","node":"plans"}))
        .err()
        .assert_value();
    assert!(
        error.contains("continuation has custom handling"),
        "{error}"
    );
}

#[test]
fn nested_custom_control_forms_block_automatic_failure_rewrites() {
    let selector = || json!({"name":"write","source":"error","field":null});
    let guarded_choice = json!({
        "kind":"choice", "name":"custom_choice", "state":{"kind":"record","fields":{}},
        "branches":[{
            "when":{"kind":"k_of_n","count":1,"values":[selector()],"labels":["crash"]},
            "node":worker("recover")
        }],
        "otherwise":done(), "promotedStatePaths":[]
    });
    let guarded_loop = json!({
        "kind":"loop", "name":"custom_loop", "state":{"kind":"record","fields":{}},
        "body":worker("retry"), "maxIterations":2, "promotedStatePaths":[],
        "until":{"kind":"not","guard":{"kind":"in","value":selector(),"labels":["crash"]}}
    });
    let guarded_parallel = json!({
        "kind":"par", "name":"custom_parallel", "state":{"kind":"record","fields":{}},
        "branches":[worker("left"),worker("right")], "promotedStatePaths":[],
        "join":{"kind":"first","when":{"kind":"in","value":selector(),"labels":["crash"]}}
    });

    for suffix in [guarded_choice, guarded_loop, guarded_parallel] {
        let error = transform(
            graph(vec![worker("write"), suffix]),
            json!({"kind":"protect","node":"write"}),
        )
        .err()
        .assert_value();
        assert!(
            error.contains("already handles this worker's outcomes"),
            "{error}"
        );
    }
}

#[test]
fn protection_accepts_one_live_choice_branch_and_rejects_terminal_targets() {
    let accepted = json!({
        "kind":"in", "value":{"name":"router","source":"signal","field":"verdict"},
        "labels":["accepted"]
    });
    let route = json!({
        "kind":"choice", "name":"route", "state":{"kind":"record","fields":{}},
        "branches":[{
            "when":accepted,
            "node":worker("continue")
        }],
        "otherwise":{"kind":"fail","name":"rejected","reason":"rejected"},
        "promotedStatePaths":[]
    });
    let protected = transform(
        graph(vec![verifier("router"), route, done()]),
        json!({"kind":"protect","node":"route"}),
    )
    .assert_value();
    assert_eq!(
        protected["graph"]["root"]["children"][2]["branches"][0]["when"]["kind"],
        "all"
    );

    for terminal in [
        done(),
        json!({"kind":"fail","name":"failed","reason":"failed"}),
    ] {
        let name = terminal["name"].as_str().assert_value();
        let error = transform(
            graph(vec![terminal.clone(), worker("after")]),
            json!({"kind":"protect","node":name}),
        )
        .err()
        .assert_value();
        assert!(error.contains("Choose an agent, worker"), "{error}");
    }
}

#[tokio::test]
async fn serial_lowering_passes_native_admission() {
    let completed = transform(
        graph(vec![worker("write")]),
        json!({"kind":"complete","owner":"run"}),
    )
    .assert_value();
    let protected = transform(
        completed["graph"].clone(),
        json!({"kind":"protect","node":"write"}),
    )
    .assert_value();
    super::super::validate_profile(
        &serde_json::from_value(protected["graph"].clone()).assert_value(),
        &serde_json::from_value(json!({
            "harness":"codex", "provider":"openai", "size":"small",
            "nodes":{"write":{"kind":"agent","model":"opaque-model"}}
        }))
        .assert_value(),
    )
    .await
    .map_err(|error| error.message)
    .assert_value();
}

#[test]
fn choice_error_policy_uses_branch_residuals_and_is_idempotent() {
    let when = json!({"kind":"in","value":{"name":"router","source":"signal","field":"verdict"},"labels":["accepted"]});
    let choice = json!({
        "kind":"choice", "name":"choose", "state":{"kind":"record","fields":{}},
        "branches":[{"when":when,"node":worker("left")}],
        "otherwise":worker("right"), "promotedStatePaths":[]
    });
    let source = graph(vec![verifier("router"), choice.clone(), done()]);
    let result = transform(source, json!({"kind":"protect","node":"choose"})).assert_value();
    assert_eq!(result["graph"]["root"]["children"][1], choice);
    let guards = &result["graph"]["root"]["children"][2]["branches"][0]["when"]["guards"];
    assert_eq!(guards[0]["guards"][0], when);
    assert_eq!(guards[0]["guards"][1]["value"]["name"], "left");
    assert_eq!(guards[1]["guards"][0], json!({"kind":"not","guard":when}));
    assert_eq!(guards[1]["guards"][1]["value"]["name"], "right");
    assert_eq!(
        transform(
            result["graph"].clone(),
            json!({"kind":"protect","node":"choose"})
        )
        .assert_value(),
        result
    );
}

#[test]
fn choice_policy_refreshes_appended_branch_and_preserves_checkpoint_identity() {
    let when = json!({"kind":"in","value":{"name":"router","source":"signal","field":"verdict"},"labels":["accepted"]});
    let choice = json!({
        "kind":"choice", "name":"choose", "state":{"kind":"record","fields":{}},
        "branches":[{"when":when,"node":worker("left")}],
        "otherwise":worker("right"), "promotedStatePaths":[]
    });
    let result = transform(
        graph(vec![verifier("router"), choice, done()]),
        json!({"kind":"protect","node":"choose"}),
    )
    .assert_value();
    let mut expanded = result["graph"].clone();
    expanded["root"]["children"][1]["branches"]
        .as_array_mut()
        .assert_value()
        .push(json!({
            "when":{
                "kind":"in",
                "value":{"name":"router","source":"signal","field":"verdict"},
                "labels":["clarify"]
            },
            "node":worker("middle")
        }));
    let refreshed = transform(expanded, json!({"kind":"protect","node":"choose"})).assert_value();
    let checkpoint = &refreshed["graph"]["root"]["children"][2];
    assert_eq!(
        checkpoint["name"],
        result["graph"]["root"]["children"][2]["name"]
    );
    assert_eq!(
        checkpoint["otherwise"],
        result["graph"]["root"]["children"][2]["otherwise"]
    );
    let terms = checkpoint["branches"][0]["when"]["guards"]
        .as_array()
        .assert_value();
    assert_eq!(terms.len(), 3);
    assert_eq!(terms[1]["guards"][2]["value"]["name"], "middle");
    assert_eq!(terms[2]["guards"].as_array().assert_value().len(), 3);
}

fn verifier(name: &str) -> Value {
    let mut node = worker(name);
    node["kind"] = json!("verifier");
    node["signals"] = json!({"verdict":["accepted","rejected"]});
    node["diagnostic"] = json!({"kind":"null"});
    node
}

async fn admit_graph(graph: Value, nodes: Value) {
    super::super::validate_profile(
        &serde_json::from_value(graph).assert_value(),
        &serde_json::from_value(
            json!({"harness":"codex","provider":"openai","size":"small","nodes":nodes}),
        )
        .assert_value(),
    )
    .await
    .map_err(|error| error.message)
    .assert_value();
}

#[tokio::test]
async fn parallel_and_map_checkpoints_pass_native_guard_availability_proofs() {
    // Step authoring is tested above; these fixtures keep guard proof independent of the
    // runtime's separately evolving writer concurrency policy.
    let source = graph(vec![
        parallel(vec![
            verifier("venue"),
            sequence("agenda_steps", vec![verifier("agenda"), verifier("budget")]),
        ]),
        done(),
    ]);
    let protected = transform(source, json!({"kind":"protect","node":"plans"})).assert_value();
    admit_graph(
        protected["graph"].clone(),
        json!({
            "venue":{"kind":"agent","model":"opaque-model"},
            "agenda":{"kind":"agent","model":"opaque-model"},
            "budget":{"kind":"agent","model":"opaque-model"}
        }),
    )
    .await;

    let state = json!({
        "kind":"record",
        "fields":{"items":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}
    });
    let mapped = json!({
        "kind":"map", "name":"items_map", "state":state, "body":verifier("draft"),
        "over":{"source":"state","path":["items"]}, "maxItems":8, "promotedStatePaths":[]
    });
    let mut source = graph(vec![mapped, done()]);
    source["initialInput"] = state.clone();
    source["root"]["state"] = state;
    let protected = transform(source, json!({"kind":"protect","node":"items_map"})).assert_value();
    admit_graph(
        protected["graph"].clone(),
        json!({"draft":{"kind":"agent","model":"opaque-model"}}),
    )
    .await;
}

#[tokio::test]
async fn protected_nested_sequence_preserves_suffix_state_promotions() {
    let state =
        json!({"kind":"record","fields":{"result":{"type":{"kind":"string"},"required":true}}});
    let mut writer = worker("write");
    writer["output"] = state.clone();
    writer["writeBindings"] =
        json!([{"target":["result"],"value":{"node":"write","channel":"out","path":["result"]}}]);
    let mut inner = sequence("inner", vec![worker("prepare"), writer]);
    inner["state"] = state.clone();
    inner["promotedStatePaths"] = json!([["result"]]);
    let mut result = done();
    result["output"] = state.clone();
    result["bindings"] =
        json!([{"target":["result"],"value":{"source":"state","path":["result"]}}]);
    let mut source = graph(vec![inner, result]);
    source["root"]["state"] = state;
    let protected = transform(source, json!({"kind":"protect","node":"prepare"})).assert_value();
    let checkpoint = &protected["graph"]["root"]["children"][0]["children"][1];
    assert_eq!(checkpoint["promotedStatePaths"], json!([["result"]]));
    assert_eq!(
        checkpoint["otherwise"]["promotedStatePaths"],
        json!([["result"]])
    );
    admit_graph(
        protected["graph"].clone(),
        json!({
            "prepare":{"kind":"agent","model":"opaque-model"},
            "write":{"kind":"agent","model":"opaque-model"}
        }),
    )
    .await;
}

#[tokio::test]
async fn protected_loop_bodies_pass_native_guaranteed_exit_and_checkpoint_proofs() {
    for (body, nodes) in [
        (
            worker("write"),
            json!({"write":{"kind":"agent","model":"opaque-model"}}),
        ),
        (
            verifier("review"),
            json!({"review":{"kind":"agent","model":"opaque-model"}}),
        ),
        (
            sequence("round", vec![worker("write"), verifier("review")]),
            json!({"write":{"kind":"agent","model":"opaque-model"},"review":{"kind":"agent","model":"opaque-model"}}),
        ),
        (
            parallel(vec![verifier("left"), verifier("right")]),
            json!({"left":{"kind":"agent","model":"opaque-model"},"right":{"kind":"agent","model":"opaque-model"}}),
        ),
    ] {
        let protected = transform(
            graph(vec![repeat(body), done()]),
            json!({"kind":"protect","node":"repeat"}),
        )
        .assert_value();
        admit_graph(protected["graph"].clone(), nodes).await;
    }
}

struct ResultWorker;
#[async_trait]
impl WorkerRegistry for ResultWorker {
    async fn resolve(&self, worker: &WorkerRef) -> Result<WorkerDescriptor, WorkerRegistryError> {
        let contract = json!({
            "input": {"kind": "null"},
            "output": result_schema(),
            "errors": ["timeout", "crash", "malformed", "refusal"]
        });
        let artifact_profile = json!({
            "allowedTypeIds": ["openengine.result@1"],
            "allowedMediaTypes": ["application/json"],
            "minimumRedaction": "internal"
        });
        serde_json::from_value(json!({
            "worker": worker,
            "graphProfiles": ["openengine.graph.full/v1"],
            "binding": {"protocol": "fixture", "version": "1", "profile": "fixture.worker/v1"},
            "contract": contract,
            "capabilityPolicy": {"autonomy": "strict", "permissionPolicy": "policy.strict@1"},
            "artifactProfile": artifact_profile,
            "credentialRequirements": []
        }))
        .map_err(|_| WorkerRegistryError::NotFound {
            worker: worker.clone(),
        })
    }
}

fn result_schema() -> Value {
    json!({"kind":"record","fields":{"result":{"type":{"kind":"string"},"required":true}}})
}

async fn protected_result_loop() -> VerifiedGraph {
    let state = result_schema();
    let mut write = worker("write");
    write["output"] = state.clone();
    write["writeBindings"] =
        json!([{"target":["result"],"value":{"node":"write","channel":"out","path":["result"]}}]);
    let mut repeated = repeat(write);
    repeated["state"] = state.clone();
    repeated["promotedStatePaths"] = json!([["result"]]);
    let mut result = done();
    result["output"] = state.clone();
    result["bindings"] =
        json!([{"target":["result"],"value":{"source":"state","path":["result"]}}]);
    let mut source = graph(vec![repeated, result]);
    source["root"]["state"] = state;
    let protected =
        transform(source, json!({"kind":"protect","node":"repeat"})).assert_value()["graph"]
            .clone();
    admit_graph(
        protected.clone(),
        json!({"write":{"kind":"agent","model":"opaque-model"}}),
    )
    .await;
    ProductionGraphVerifier::new(ResultWorker)
        .verify(&serde_json::from_value(protected).assert_value())
        .await
        .assert_value()
}

fn settled_round(id: u64, outcome: WorkerOutcome) -> DurableExecution {
    DurableExecution {
        dispatch_position: HistoryPosition::new(id * 2 - 1).assert_value(),
        node_instance: NodeInstanceId::new(1).assert_value(),
        execution: ExecutionId::new(id).assert_value(),
        occurrence: StructuralOccurrence {
            node: NodeName::new("write").assert_value(),
            map_indices: Vec::new(),
        },
        attempt: PositiveInteger::new(1).assert_value(),
        input: Value::Null,
        state: DurableExecutionState::Settled {
            position: HistoryPosition::new(id * 2).assert_value(),
            outcome,
        },
    }
}

fn successful_round(id: u64) -> DurableExecution {
    settled_round(
        id,
        WorkerOutcome::Verified {
            output: json!({"result":format!("round {id}")}),
            artifacts: Vec::new(),
        },
    )
}

fn reduce_rounds(graph: &VerifiedGraph, history: &[DurableExecution]) -> Reduction {
    let result = FullV1Reducer::native_v2(graph).reduce(ReductionInput {
        initial_input: &json!({}),
        executions: history,
        next_node_instance: history.len() as u64 + 1,
        next_execution: history.len() as u64 + 1,
    });
    assert!(result.is_ok(), "{result:?}");
    result.assert_value()
}

#[tokio::test]
async fn protected_fixed_count_repetition_finishes_all_successful_rounds_with_latest_result() {
    let graph = protected_result_loop().await;
    let mut history = Vec::new();
    for id in 1..=3 {
        let before = reduce_rounds(&graph, &history);
        assert!(before.terminal.is_none());
        assert!(before.decisions.iter().any(
            |decision| matches!(decision, Decision::Dispatch { occurrence, .. }
                if occurrence.node.as_str() == "write")
        ));
        history.push(successful_round(id));
    }
    let completed = reduce_rounds(&graph, &history);
    assert!(matches!(
        completed.terminal,
        Some(TerminalProjection::Succeeded { output, .. })
            if output == json!({"result":"round 3"})
    ));
}

#[tokio::test]
async fn protected_fixed_count_repetition_fails_first_bad_round_including_final_round() {
    let graph = protected_result_loop().await;
    for error in [
        WorkerErrorCode::Crash,
        WorkerErrorCode::Malformed,
        WorkerErrorCode::Refusal,
        WorkerErrorCode::Timeout,
    ] {
        for failed_round in 1..=3 {
            let mut history = (1..failed_round).map(successful_round).collect::<Vec<_>>();
            history.push(settled_round(
                failed_round,
                WorkerOutcome::declared_failure(error),
            ));
            let failed = reduce_rounds(&graph, &history);
            assert!(matches!(
                failed.terminal,
                Some(TerminalProjection::Failed { reason, .. })
                    if reason.as_str() == "execution_failed"
            ));
            assert!(
                !failed
                    .decisions
                    .iter()
                    .any(|decision| matches!(decision, Decision::Dispatch { .. }))
            );
            assert!(!failed.decisions.iter().any(|decision| matches!(
                decision,
                Decision::Terminal {
                    projection: TerminalProjection::Succeeded { .. }
                }
            )));
        }
    }
}
