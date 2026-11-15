use super::*;

#[tokio::test]
async fn first_presence_and_aggregate_correlations_admit_reachable_paths() {
    for (index, source) in [
        include_str!("join-fixtures/conditional-first.json"),
        include_str!("join-fixtures/conditional-first-nested-all.json"),
        include_str!("join-fixtures/conditional-first-nested-any.json"),
        include_str!("join-fixtures/conditional-first-nested-first.json"),
        include_str!("join-fixtures/no-satisfier-terminal.json"),
        include_str!("join-fixtures/group-label-class.json"),
        include_str!("join-fixtures/mapped-absent-retention.json"),
    ]
    .into_iter()
    .enumerate()
    {
        let graph: GraphSpec = serde_json::from_str(source).assert_value();
        let result = validate_join_graph(graph).await;
        assert!(
            result.is_ok(),
            "reachable fixture {index} rejected: {result:?}"
        );
    }
}

#[tokio::test]
async fn first_cannot_combine_controls_of_mutually_exclusive_inner_winners() {
    let mut source: serde_json::Value = serde_json::from_str(include_str!(
        "join-fixtures/conditional-first-nested-any.json"
    ))
    .assert_value();
    source["root"]["children"][0]["join"]["when"] = json!({"kind":"all","guards":[
        {"kind":"in","value":{"name":"conditional","source":"signal","field":"verdict"},"labels":["accepted"]},
        {"kind":"in","value":{"name":"innerOther","source":"signal","field":"verdict"},"labels":["accepted"]}]});
    for source in [
        source,
        serde_json::from_str(include_str!(
            "join-fixtures/impossible-first-subset-racer-root.json"
        ))
        .assert_value(),
    ] {
        let graph: GraphSpec = serde_json::from_value(source).assert_value();
        let result = validate_join_graph(graph).await;
        assert!(
            matches!(&result, Err(NativeV2AdmissionError::GraphVerification(
                openengine_cluster_server::admission::VerificationError::Rejected { diagnostics }
            )) if diagnostics.iter().any(|diagnostic| match diagnostic.code {
                openengine_cluster_protocol::GraphDiagnosticCode::InvalidGraphShape =>
                    diagnostic.message == "first join guard cannot be established by any candidate racer",
                openengine_cluster_protocol::GraphDiagnosticCode::UndefinedRead |
                openengine_cluster_protocol::GraphDiagnosticCode::ChoiceExhaustiveness => true,
                _ => false,
            })),
            "exclusive winner evidence did not produce a control/dataflow rejection: {result:?}"
        );
    }
}

async fn validate_join_graph(graph: GraphSpec) -> Result<(), NativeV2AdmissionError> {
    let nodes = executable_declarations(&graph.root)
        .into_iter()
        .map(|declaration| (declaration.name, binding("opaque", None)))
        .collect();
    let request = submission(graph, nodes);
    NativeV2Admission
        .validate_profile(&request.graph, &request.runtime, DeliveryPolicy::Optional)
        .await
}
