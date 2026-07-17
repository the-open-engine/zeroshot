#[tokio::test]
async fn map_promotes_indexed_results_and_defines_empty_results() {
    let graph = indexed_map_graph(
        json!({"kind":"integer"}),
        json!({"kind":"array","items":{"kind":"integer"}}),
        true,
    );

    ProductionGraphVerifier::new(registry())
        .verify(&graph)
        .await
        .unwrap();
}

#[tokio::test]
async fn map_indexed_promotions_reject_invalid_element_sources_and_targets() {
    let mismatch = indexed_map_graph(
        json!({"kind":"number"}),
        json!({"kind":"array","items":{"kind":"integer"}}),
        true,
    );
    let mismatch = ProductionGraphVerifier::new(registry())
        .verify(&mismatch)
        .await
        .unwrap_err();
    let mismatch_codes = rejection_codes(mismatch);
    assert!(mismatch_codes.contains(&GraphDiagnosticCode::SchemaSafety));
    assert!(!mismatch_codes.contains(&GraphDiagnosticCode::UndefinedRead));

    let scalar_target =
        indexed_map_graph(json!({"kind":"integer"}), json!({"kind":"integer"}), true);
    let mut scalar_target = serde_json::to_value(scalar_target).unwrap();
    scalar_target["root"]["children"][1] =
        json!({"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]});
    let scalar_target = serde_json::from_value(scalar_target).unwrap();
    let scalar_target = ProductionGraphVerifier::new(registry())
        .verify(&scalar_target)
        .await
        .unwrap_err();
    let scalar_codes = rejection_codes(scalar_target);
    assert!(scalar_codes.contains(&GraphDiagnosticCode::SchemaSafety));
    assert!(!scalar_codes.contains(&GraphDiagnosticCode::UndefinedRead));

    let missing_writer = indexed_map_graph(
        json!({"kind":"integer"}),
        json!({"kind":"array","items":{"kind":"integer"}}),
        false,
    );
    let missing_writer = ProductionGraphVerifier::new(registry())
        .verify(&missing_writer)
        .await
        .unwrap_err();
    assert!(rejection_codes(missing_writer).contains(&GraphDiagnosticCode::UndefinedRead));
}

#[tokio::test]
async fn map_indexed_reads_do_not_reuse_the_outer_array_definition() {
    let mut value = serde_json::to_value(indexed_map_graph(
        json!({"kind":"integer"}),
        json!({"kind":"array","items":{"kind":"integer"}}),
        true,
    ))
    .unwrap();
    value["initialInput"]["fields"]["results"]["required"] = json!(true);
    value["root"]["children"][0]["body"]["input"]["fields"]["prior"] =
        json!({"type":{"kind":"integer"},"required":true});
    value["root"]["children"][0]["body"]["inputBindings"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "target":["prior"],
            "value":{"source":"state","path":["results"]}
        }));
    let error = ProductionGraphVerifier::new(registry())
        .verify(&serde_json::from_value(value).unwrap())
        .await
        .unwrap_err();
    let codes = rejection_codes(error);
    assert!(codes.contains(&GraphDiagnosticCode::UndefinedRead));
    assert!(!codes.contains(&GraphDiagnosticCode::SchemaSafety));
}
use super::map_fixture::indexed_map_graph;
use super::*;
