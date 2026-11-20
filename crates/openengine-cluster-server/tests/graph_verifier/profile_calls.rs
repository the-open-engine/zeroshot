use super::*;

fn resolved_profile(input: Value, body: Value) -> Value {
    json!({
        "kind": "profile",
        "name": "call",
        "profile": "child",
        "initialInput": input,
        "body": body
    })
}

fn sequence(name: &str, state: Value, children: Vec<Value>) -> Value {
    json!({
        "kind": "seq", "name": name, "state": state, "children": children, "promotedStatePaths": []
    })
}

fn item_map(name: &str, body: Value) -> Value {
    json!({
        "kind": "map", "name": name, "state": state(), "body": body, "maxItems": 2,
        "over": {"source": "state", "path": ["items"]}, "promotedStatePaths": []
    })
}

fn aggregate_choice(name: &str, selector: &str) -> Value {
    json!({
        "kind": "choice", "name": name, "state": state(),
        "branches": [{
            "when": {
                "kind": "k_of_map", "count": 1,
                "value": {"source": "signal", "name": selector, "field": "verdict"},
                "labels": ["accepted"]
            },
            "node": succeed(&format!("{name}_matched"))
        }],
        "otherwise": succeed(&format!("{name}_unmatched")),
        "promotedStatePaths": []
    })
}

#[tokio::test]
async fn profile_continues_the_caller_and_counts_child_work() {
    let input = json!({"kind": "record", "fields": {}});
    let child_state = json!({
        "kind": "record", "fields": {"scratch": {"type": {"kind": "string"}, "required": true}}
    });
    let child_terminal = json!({
        "kind": "succeed", "name": "child_done", "output": child_state,
        "bindings": [{"target": ["scratch"], "value": {"source": "state", "path": ["scratch"]}}]
    });
    let child = sequence(
        "child",
        child_state,
        vec![null_step("child_work", 2), child_terminal],
    );
    let graph = graph(vec![
        item_map("outer", resolved_profile(input, child)),
        null_step("parent_work", 1),
        succeed("parent_done"),
    ]);

    let verified = verify(&graph).await.assert_value();
    assert_eq!(verified.compiled_ir.bounds.peak_concurrency.get(), 2);
    assert_eq!(verified.compiled_ir.bounds.attempts_per_node.len(), 2);
    assert!(
        verified
            .compiled_ir
            .bounds
            .attempts_per_node
            .iter()
            .any(|(name, attempts)| { name.as_str() == "child_work" && attempts.get() == 2 })
    );
}

async fn assert_unreachable(graph: &GraphSpec, node: &str) {
    let name = node.parse().assert_value();
    let error = diagnostics(verify(graph).await.assert_error());
    assert!(
        error.iter().any(|diagnostic| {
            diagnostic.code == GraphDiagnosticCode::Reachability
                && diagnostic.related_nodes.contains(&name)
        }),
        "{error:?}"
    );
}

#[tokio::test]
async fn profile_calls_require_complete_resolution_and_a_separate_child_terminal() {
    let unresolved = graph(vec![
        json!({"kind": "profile", "name": "call", "profile": "child"}),
        succeed("parent_done"),
    ]);
    let error = diagnostics(verify(&unresolved).await.assert_error());
    assert!(error.iter().any(|diagnostic| {
        diagnostic.code == GraphDiagnosticCode::InvalidGraphShape
            && diagnostic
                .message
                .contains("resolved body and initialInput")
    }));

    let falls_through = graph(vec![
        resolved_profile(state(), null_step("child_work", 1)),
        succeed("parent_done"),
    ]);
    assert_unreachable(&falls_through, "child_work").await;
}

#[tokio::test]
async fn profile_input_must_be_available_in_caller_state() {
    let input = json!({
        "kind": "record", "fields": {"missing": {"type": {"kind": "string"}, "required": true}}
    });
    let graph = graph(vec![
        resolved_profile(input, succeed("child_done")),
        succeed("parent_done"),
    ]);
    let error = diagnostics(verify(&graph).await.assert_error());
    assert!(error.iter().any(|diagnostic| {
        diagnostic.code == GraphDiagnosticCode::SchemaSafety
            && diagnostic
                .message
                .contains("cannot supply the profile's initialInput")
    }));
}

#[tokio::test]
async fn profile_keeps_aggregate_selectors_inside_their_scope() {
    let child = sequence(
        "child",
        state(),
        vec![
            item_map("inner", null_verifier("private_check")),
            aggregate_choice("child_check", "private_check"),
        ],
    );
    let accepted = graph(vec![
        item_map("outer", resolved_profile(state(), child.clone())),
        succeed("parent_done"),
    ]);
    verify(&accepted).await.assert_value();

    let rejected = graph(vec![
        item_map("outer", resolved_profile(state(), child)),
        aggregate_choice("parent_check", "private_check"),
    ]);
    let error = diagnostics(verify(&rejected).await.assert_error());
    assert!(
        error.iter().any(|diagnostic| {
            diagnostic.code == GraphDiagnosticCode::UndefinedRead
                && diagnostic.message.contains("private_check")
        }),
        "{error:?}"
    );
}

#[tokio::test]
async fn imported_loop_limit_must_be_declared_in_both_initial_inputs() {
    let mut run_schema = state();
    run_schema
        .get_mut("fields")
        .assert_value()
        .as_object_mut()
        .assert_value()
        .insert(
            "limit".to_owned(),
            json!({"type": {"kind": "integer"}, "required": true}),
        );
    let body = sequence(
        "child",
        state(),
        vec![
            json!({
                "kind": "loop", "name": "repeat", "state": state(),
                "body": null_step("work", 1), "maxIterations": 2,
                "maxIterationsInput": ["limit"], "promotedStatePaths": []
            }),
            succeed("child_done"),
        ],
    );
    let graph_with_call = |child_schema| {
        serde_json::from_value(json!({
            "profile": "openengine.graph.full/v1",
            "policy": {"policy": "policy.strict@1", "default": "deny"},
            "initialInput": run_schema,
            "root": sequence(
                "root",
                run_schema.clone(),
                vec![resolved_profile(child_schema, body.clone()), succeed("parent_done")],
            ),
        }))
        .assert_value()
    };

    verify(&graph_with_call(run_schema.clone()))
        .await
        .assert_value();
    let error = diagnostics(verify(&graph_with_call(state())).await.assert_error());
    assert!(error.iter().any(|diagnostic| {
        diagnostic.code == GraphDiagnosticCode::SchemaSafety
            && diagnostic
                .message
                .contains("the enclosing profile's initialInput")
    }));
}

#[tokio::test]
async fn nested_profile_success_does_not_make_a_failing_caller_return() {
    let child = sequence(
        "child",
        state(),
        vec![
            json!({
                "kind": "profile", "name": "nested_call", "profile": "leaf",
                "initialInput": state(), "body": succeed("nested_done")
            }),
            json!({"kind": "fail", "name": "child_failed", "reason": "blocked"}),
        ],
    );
    let graph = graph(vec![
        resolved_profile(state(), child),
        null_step("unreachable_parent_work", 1),
        succeed("parent_done"),
    ]);
    assert_unreachable(&graph, "unreachable_parent_work").await;
}
