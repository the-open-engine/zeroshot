use super::*;

fn window_empty() -> Value {
    json!({"kind":"record","fields":{"empty":{"type":{"kind":"array","items":{"kind":"null"}},"required":true}}})
}
fn window_seq(name: &str, state: &Value, nodes: Vec<Value>) -> Value {
    if nodes.is_empty() {
        return json!({"kind":"map","name":name,"state":state,"over":{"source":"state","path":["empty"]},"maxItems":1,"promotedStatePaths":[],"body":step(&format!("{name}_unused"),1)});
    }
    json!({"kind":"seq","name":name,"state":state,"children":nodes,"promotedStatePaths":[]})
}
fn window_par(name: &str, state: &Value, nodes: Vec<Value>, join: Value) -> Value {
    json!({"kind":"par","name":name,"state":state,"branches":nodes,"join":join,"promotedStatePaths":[]})
}
fn window_loop(state: &Value, body: Value) -> Value {
    json!({"kind":"loop","name":"twice","state":state,"body":body,"maxIterations":2,"promotedStatePaths":[]})
}
fn window_signal(name: &str) -> Value {
    json!({"kind":"in","value":{"name":name,"source":"signal","field":"verdict"},"labels":["accepted"]})
}
fn window_route(state: &Value, gate: &str) -> Value {
    json!({"kind":"choice","name":"gate_route","state":state,"branches":[{
        "when":window_signal(gate),"node":window_seq("allowed",state,vec![])}],
        "otherwise":{"kind":"fail","name":"denied","reason":"denied"},"promotedStatePaths":[]})
}
struct WindowWorkers;
#[async_trait::async_trait]
impl openengine_cluster_server::worker_registry::WorkerRegistry for WindowWorkers {
    async fn resolve(
        &self,
        worker: &openengine_cluster_protocol::WorkerRef,
    ) -> Result<
        openengine_cluster_protocol::WorkerDescriptor,
        openengine_cluster_server::worker_registry::WorkerRegistryError,
    > {
        let descriptor = TestWorkers { rich_outputs: true }.resolve(worker).await?;
        let mut value = serde_json::to_value(descriptor).unwrap();
        if worker.as_str() == "worker.bound@1" {
            value["contract"]["input"] = json!({"kind":"record","fields":{"value":{"type":{"kind":"integer"},"required":true}}});
        }
        Ok(serde_json::from_value(value).unwrap())
    }
}
async fn window_graph(state: &Value, root: Value) -> VerifiedGraph {
    let graph: GraphSpec = serde_json::from_value(
        json!({"profile":"openengine.graph.full/v1","initialInput":state,
        "policy":{"policy":"policy.test@1","default":"deny"},"root":root}),
    )
    .unwrap();
    ProductionGraphVerifier::new(WindowWorkers)
        .verify(&graph)
        .await
        .unwrap()
}
async fn repeated_graph(state: &Value, body: Value) -> VerifiedGraph {
    window_graph(
        state,
        window_seq(
            "root",
            state,
            vec![window_loop(state, body), succeed("done")],
        ),
    )
    .await
}

fn window_run(
    graph: &VerifiedGraph,
    input: &Value,
    history: &[DurableExecution],
    capacity: usize,
) -> Result<Reduction, ReducerError> {
    FullV1Reducer::native_v2(graph)
        .with_dispatch_limit(capacity)
        .reduce(ReductionInput {
            initial_input: input,
            executions: history,
            next_node_instance: history
                .iter()
                .map(|e| e.node_instance.get())
                .max()
                .unwrap_or(0)
                + 1,
            next_execution: history.iter().map(|e| e.execution.get()).max().unwrap_or(0) + 1,
        })
}
#[tokio::test]
async fn impossible_joins_ignore_later_round_history() {
    let s = window_empty();
    for join in [json!({"kind":"all"}), json!({"kind":"quorum","count":2})] {
        let route = json!({"kind":"choice","name":"route","state":s,"branches":[{
            "when":window_signal("a"),"node":{"kind":"fail","name":"stop","reason":"stop"}}],
            "otherwise":window_seq("pass",&s,vec![]),"promotedStatePaths":[]});
        let body = window_par(
            "race",
            &s,
            vec![
                window_seq("a_branch", &s, vec![verifier("a", 1), route]),
                step("b", 1),
            ],
            join.clone(),
        );
        let graph = repeated_graph(&s, body).await;
        let history = vec![
            settled(SettledSpec::new(1, 1, "a").position(2), verdict("accepted")),
            settled(SettledSpec::new(2, 2, "b").position(4), success(1)),
            settled(SettledSpec::new(3, 1, "a").position(6), verdict("rejected")),
        ];
        let mut active = history.clone();
        active[1].state = DurableExecutionState::Active;
        let waiting = window_run(&graph, &json!({"empty":[]}), &active, 1).unwrap();
        assert!(waiting.terminal.is_none());
        assert!(!waiting.decisions.iter().any(|decision| matches!(decision,
            Decision::VoidLoser { execution, .. } if execution.get() == 2)));
        assert!(!waiting.decisions.iter().any(|decision| matches!(decision,
            Decision::Dispatch { occurrence, .. } if occurrence.node.as_str() == "b")));
        let result = window_run(&graph, &json!({"empty":[]}), &history, 1).unwrap();
        assert!(matches!(
            result.terminal,
            Some(TerminalProjection::Succeeded { .. })
        ));
        assert!(
            !result
                .decisions
                .iter()
                .any(|d| matches!(d, Decision::Dispatch { .. }))
        );
    }
}
#[tokio::test]
async fn changing_inputs_belong_to_their_own_round() {
    let s = json!({"kind":"record","fields":{"value":{"type":{"kind":"integer"},"required":true}}});
    let mut b = step("b", 1);
    b["worker"] = json!("worker.bound@1");
    b["input"] = s.clone();
    b["inputBindings"] = json!([{"target":["value"],"value":{"source":"state","path":["value"]}}]);
    let race = window_par("race", &s, vec![verifier("a", 1), b], json!({"kind":"any"}));
    let mut body = window_seq(
        "round",
        &s,
        vec![race, promoted_integer_step("change", "value")],
    );
    body["promotedStatePaths"] = json!([["value"]]);
    let graph = repeated_graph(&s, body).await;
    let initial = json!({"value":1});
    for active_later in [true, false] {
        let mut later = active(3, 3, "b", 5);
        later.input = json!({"value":2});
        if !active_later {
            later.state = DurableExecutionState::Settled {
                position: HistoryPosition::new(6).unwrap(),
                outcome: success(1),
            };
        }
        let history = vec![
            settled(SettledSpec::new(1, 1, "a").position(2), verdict("accepted")),
            settled(SettledSpec::new(2, 2, "change").position(4), success(2)),
            later,
        ];
        let result = window_run(&graph, &initial, &history, 1);
        let result = result.unwrap();
        assert!(
            !result.decisions.iter().any(
                |d| matches!(d, Decision::VoidLoser { execution, .. } if execution.get() == 3)
            )
        );
        assert!(result.decisions.iter().any(|decision| matches!(decision,
            Decision::Dispatch { occurrence, iteration, .. } if *iteration == 2 && occurrence.node.as_str() == if active_later { "a" } else { "change" })));
    }
}
#[tokio::test]
async fn terminal_maps_preserve_execution_progress() {
    let s = required_array_record(&[("items", "null")]);
    let map = json!({"kind":"map","name":"mapped","state":s,"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[],
        "body":window_seq("item",&s,vec![step("work",1),json!({"kind":"fail","name":"end_item","reason":"end_item"})])});
    let body = window_par(
        "race",
        &s,
        vec![map, step("other", 1)],
        json!({"kind":"all"}),
    );
    let graph = repeated_graph(&s, body).await;
    let history = vec![settled(
        SettledSpec::new(1, 1, "work")
            .position(2)
            .map_indices(vec![0]),
        success(1),
    )];
    let result = window_run(&graph, &json!({"items":[null]}), &history, 2).unwrap();
    assert!(result.terminal.is_none());
    assert!(result.decisions.iter().any(|d|matches!(d,Decision::Dispatch{occurrence, iteration, ..} if occurrence.node.as_str() == "work" && *iteration == 2)));
}
#[tokio::test]
async fn item_local_map_guards_qualify_first_joins() {
    let s = required_array_record(&[("items", "null")]);
    let guard = json!({"kind":"k_of_map","count":1,"value":{"name":"a","source":"signal","field":"verdict"},"labels":["accepted"]});
    let race = window_par(
        "race",
        &s,
        vec![verifier("a", 1), verifier("b", 1)],
        json!({"kind":"first","when":guard}),
    );
    let map = json!({"kind":"map","name":"mapped","state":s,"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[],"body":race});
    let graph = window_graph(&s, window_seq("root", &s, vec![map, succeed("done")])).await;
    let history = vec![settled(
        SettledSpec::new(1, 1, "a").position(2).map_indices(vec![0]),
        verdict("accepted"),
    )];
    let result = window_run(&graph, &json!({"items":[null]}), &history, 1).unwrap();
    assert!(matches!(
        result.terminal,
        Some(TerminalProjection::Succeeded { .. })
    ));
}
fn window_known(name: &str) -> Value {
    let atom =
        json!({"kind":"in","value":{"name":name,"source":"error","field":null},"labels":["crash"]});
    json!({"kind":"any","guards":[atom,{"kind":"not","guard":atom}]})
}
#[tokio::test]
async fn incoming_controls_preserve_join_retention() {
    let s = window_empty();
    let prior = window_par(
        "prior",
        &s,
        vec![
            verifier("priorVerify", 1),
            window_seq(
                "right",
                &s,
                vec![verifier("gate", 1), window_route(&s, "gate")],
            ),
        ],
        json!({"kind":"all"}),
    );
    let when = json!({"kind":"all","guards":[window_known("priorVerify"),window_known("current")]});
    let next = window_par(
        "next",
        &s,
        vec![verifier("current", 1)],
        json!({"kind":"first","when":when}),
    );
    let route = json!({"kind":"choice","name":"outcome","state":s,"branches":[{"when":{"kind":"in","value":{"name":"next","source":"group","field":"raced"},"labels":["satisfied"]},"node":succeed("done")}],"promotedStatePaths":[]});
    let mut root = window_seq("root", &s, vec![prior, next, route]);
    assert!(
        window_verification(&s, root.clone()).await.is_err(),
        "unsafe continuation must be rejected"
    );
    root["children"][2]["otherwise"] =
        json!({"kind":"fail","name":"no_controls","reason":"no_retained_controls"});
    let safe = window_verification(&s, root).await.unwrap();
    let history = [
        settled(
            SettledSpec::new(1, 1, "priorVerify").position(2),
            verdict("rejected"),
        ),
        settled(
            SettledSpec::new(2, 2, "gate").position(4),
            verdict("rejected"),
        ),
        settled(
            SettledSpec::new(3, 3, "current").position(6),
            verdict("accepted"),
        ),
    ];
    let reduced = window_run(&safe, &json!({"empty":[]}), &history, 1).unwrap();
    assert!(
        matches!(reduced.terminal, Some(TerminalProjection::Failed { reason, .. }) if reason.as_str() == "no_retained_controls")
    );
}
#[tokio::test]
async fn mapped_writes_require_join_retention() {
    let mut s =
        required_array_record(&[("items", "null"), ("empty", "null"), ("results", "integer")]);
    s["fields"]["results"]["required"] = json!(false);
    let mut body = window_par(
        "joined",
        &s,
        vec![
            promoted_integer_step("writer", "results"),
            window_seq(
                "right",
                &s,
                vec![verifier("gate", 1), window_route(&s, "gate")],
            ),
        ],
        json!({"kind":"all"}),
    );
    body["promotedStatePaths"] = json!([["results"]]);
    let map = json!({"kind":"map","name":"mapped","state":s,"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[["results"]],"body":body});
    let read = json!({"kind":"succeed","name":"read","output":required_array_record(&[("results","integer")]),"bindings":[{"target":["results"],"value":{"source":"state","path":["results"]}}]});
    let route = json!({"kind":"choice","name":"outcome","state":s,"branches":[{"when":{"kind":"not","guard":{"kind":"k_of_map","count":1,"value":{"name":"writer","source":"error","field":null},"labels":["timeout","crash","malformed","refusal"]}},"node":read}],"otherwise":{"kind":"fail","name":"bad_writer","reason":"bad_writer"},"promotedStatePaths":[]});
    let mut root = window_seq("root", &s, vec![map, route]);
    assert!(
        window_verification(&s, root.clone()).await.is_err(),
        "unsafe continuation must be rejected"
    );
    let writer_ok = root["children"][1]["branches"][0]["when"].take();
    root["children"][1]["branches"][0]["when"] = json!({"kind":"all","guards":[writer_ok,
        {"kind":"not","guard":window_kmap("joined","group",Some("joined"),vec!["quorum_unreachable"])}]});
    let safe = window_verification(&s, root).await.unwrap();
    let input = json!({"items":[null],"empty":[]});
    for label in ["accepted", "rejected"] {
        let history = [
            settled(
                SettledSpec::new(1, 1, "writer")
                    .position(2)
                    .map_indices(vec![0]),
                success(7),
            ),
            settled(
                SettledSpec::new(2, 2, "gate")
                    .position(4)
                    .map_indices(vec![0]),
                verdict(label),
            ),
        ];
        let reduced = window_run(&safe, &input, &history, 1).unwrap();
        if label == "accepted" {
            assert_eq!(
                reduced.terminal,
                Some(TerminalProjection::Succeeded {
                    output: json!({"results":[7]})
                })
            );
        } else {
            assert!(matches!(
                reduced.terminal,
                Some(TerminalProjection::Failed { .. })
            ));
        }
    }
    assert_eq!(
        window_run(&safe, &json!({"items":[],"empty":[]}), &[], 1)
            .unwrap()
            .terminal,
        Some(TerminalProjection::Succeeded {
            output: json!({"results":[]})
        })
    );
}
async fn window_verification(
    state: &Value,
    root: Value,
) -> Result<VerifiedGraph, openengine_cluster_server::admission::VerificationError> {
    let graph:GraphSpec=serde_json::from_value(json!({"profile":"openengine.graph.full/v1","initialInput":state,"policy":{"policy":"policy.test@1","default":"deny"},"root":root})).unwrap();
    ProductionGraphVerifier::new(WindowWorkers)
        .verify(&graph)
        .await
}
fn window_kmap(name: &str, source: &str, field: Option<&str>, labels: Vec<&str>) -> Value {
    json!({"kind":"k_of_map","count":1,"value":{"name":name,"source":source,"field":field},"labels":labels})
}
#[tokio::test]
async fn nested_maps_discard_losing_counts() {
    let s = required_array_record(&[("items", "null")]);
    let inner = json!({"kind":"map","name":"inner","state":s,"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[],"body":verifier("iv",1)});
    let race = window_par(
        "race",
        &s,
        vec![inner, verifier("right", 1)],
        json!({"kind":"first","when":window_signal("right")}),
    );
    let outer = json!({"kind":"map","name":"outer","state":s,"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[],"body":race});
    let route = json!({"kind":"choice","name":"outcome","state":s,"branches":[{"when":{"kind":"not","guard":window_kmap("iv","signal",Some("verdict"),vec!["accepted"])},"node":succeed("done")}],"promotedStatePaths":[]});
    let result = window_verification(&s, window_seq("root", &s, vec![outer, route])).await;
    result.unwrap();
}
#[tokio::test]
async fn external_gates_preserve_one_map_population() {
    let mut s = required_array_record(&[("items", "null")]);
    s["fields"]["empty"] = window_empty()["fields"]["empty"].clone();
    let body = json!({"kind":"choice","name":"per_item","state":s,"branches":[{"when":window_signal("gate"),"node":window_seq("both",&s,vec![verifier("a",1),verifier("b",1)])}],"otherwise":window_seq("neither",&s,vec![]),"promotedStatePaths":[]});
    let map = json!({"kind":"map","name":"mapped","state":s,"over":{"source":"state","path":["items"]},"maxItems":1,"promotedStatePaths":[],"body":body});
    let guard = json!({"kind":"any","guards":[{"kind":"not","guard":window_kmap("a","signal",Some("verdict"),vec!["accepted","rejected"])},window_kmap("b","signal",Some("verdict"),vec!["accepted","rejected"]),window_kmap("b","error",None,vec!["timeout","crash","malformed","refusal"])]});
    let route = json!({"kind":"choice","name":"outcome","state":s,"branches":[{"when":guard,"node":succeed("done")}],"promotedStatePaths":[]});
    let result = window_verification(
        &s,
        window_seq("root", &s, vec![verifier("gate", 1), map, route]),
    )
    .await;
    result.unwrap();
}
#[test]
fn concurrency_decoding_matches_the_schema_bound() {
    for value in [0, 65_537, 100_000] {
        assert!(
            serde_json::from_value::<openengine_cluster_protocol::RuntimePlan>(
                json!({"harness":"codex","provider":"openai","maxConcurrency":value,"nodes":{}})
            )
            .is_err()
        );
    }
    for value in [1, 64, 65_536] {
        let runtime: openengine_cluster_protocol::RuntimePlan = serde_json::from_value(
            json!({"harness":"codex","provider":"openai","maxConcurrency":value,"nodes":{}}),
        )
        .unwrap();
        assert_eq!(runtime.effective_max_concurrency(), value);
    }
}

#[tokio::test]
async fn only_actual_loop_entries_charge_the_ceiling() {
    let s = window_empty();
    let before = json!({"kind":"loop","name":"before","state":s,"body":window_seq("before_empty",&s,vec![]),"maxIterations":1,"promotedStatePaths":[]});
    let inner = json!({"kind":"loop","name":"empty_rounds","state":s,"body":window_seq("inner_empty",&s,vec![]),"maxIterations":65533,"promotedStatePaths":[]});
    let left = window_seq("left", &s, vec![inner, step("b", 1)]);
    let race = window_par("race", &s, vec![left, step("a", 1)], json!({"kind":"any"}));
    let graph = window_graph(
        &s,
        window_seq(
            "root",
            &s,
            vec![before, window_loop(&s, race), succeed("done")],
        ),
    )
    .await;
    let history = vec![
        settled(SettledSpec::new(1, 1, "a").position(2), success(1)),
        active(2, 2, "b", 3),
    ];
    let result = window_run(&graph, &json!({"empty":[]}), &history, 1);
    let result = result.unwrap();
    assert!(result.terminal.is_none());
    assert!(result.decisions.iter().any(|decision| matches!(decision,
        Decision::Dispatch { occurrence, iteration, .. } if occurrence.node.as_str() == "a" && *iteration == 2)));
}

#[tokio::test]
async fn complete_history_replays_at_the_supported_visit_ceiling() {
    let state = json!({"kind":"record","fields":{}});
    let repeated = json!({"kind":"loop","name":"repeat","state":state,"maxIterations":65536,
        "body":step("work", 1),"promotedStatePaths":[]});
    let graph = window_graph(
        &state,
        window_seq("root", &state, vec![repeated, succeed("done")]),
    )
    .await;
    let history = (1..=65_536)
        .map(|id| settled(SettledSpec::new(id, 1, "work").position(id * 2), success(1)))
        .collect::<Vec<_>>();
    let reduced = window_run(&graph, &json!({}), &history, 1).unwrap();
    assert!(matches!(
        reduced.terminal,
        Some(TerminalProjection::Succeeded { .. })
    ));
    assert!(
        !reduced
            .decisions
            .iter()
            .any(|decision| matches!(decision, Decision::Dispatch { .. }))
    );
}
