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

use super::*;
