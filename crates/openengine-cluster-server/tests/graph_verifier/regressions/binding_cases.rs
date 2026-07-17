#[tokio::test]
async fn field_bindings_reject_unconstructible_root_payloads_before_registry_resolution() {
    let payloads = [
        json!({"kind":"boolean"}),
        json!({"kind":"integer"}),
        json!({"kind":"number"}),
        json!({"kind":"string"}),
        json!({"kind":"enum","values":["one"]}),
        json!({"kind":"array","items":{"kind":"integer"}}),
    ];

    for payload in payloads {
        let mut worker_value = valid_graph();
        worker_value["root"]["children"][0]["input"] = payload.clone();
        worker_value["root"]["children"][0]["inputBindings"] = json!([]);
        let worker_graph: GraphSpec = serde_json::from_value(worker_value).unwrap();
        let worker_registry = registry();
        let worker_resolutions = Arc::clone(&worker_registry.resolutions);
        let worker_error = ProductionGraphVerifier::new(worker_registry)
            .verify(&worker_graph)
            .await
            .unwrap_err();
        assert_eq!(worker_resolutions.load(Ordering::Relaxed), 0);
        assert!(has_schema_diagnostic_at_field(&worker_error, "input"));

        let mut verifier_value = valid_graph();
        verifier_value["root"]["children"][1]["input"] = payload.clone();
        verifier_value["root"]["children"][1]["inputBindings"] = json!([]);
        let verifier_graph: GraphSpec = serde_json::from_value(verifier_value).unwrap();
        let verifier_registry = registry();
        let verifier_resolutions = Arc::clone(&verifier_registry.resolutions);
        let verifier_error = ProductionGraphVerifier::new(verifier_registry)
            .verify(&verifier_graph)
            .await
            .unwrap_err();
        assert_eq!(verifier_resolutions.load(Ordering::Relaxed), 0);
        assert!(has_schema_diagnostic_at_field(&verifier_error, "input"));

        let mut terminal_value = valid_graph();
        terminal_value["root"]["children"][2]["branches"][0]["node"]["output"] = payload;
        terminal_value["root"]["children"][2]["branches"][0]["node"]["bindings"] = json!([]);
        let terminal_graph: GraphSpec = serde_json::from_value(terminal_value).unwrap();
        let terminal_registry = registry();
        let terminal_resolutions = Arc::clone(&terminal_registry.resolutions);
        let terminal_error = ProductionGraphVerifier::new(terminal_registry)
            .verify(&terminal_graph)
            .await
            .unwrap_err();
        assert_eq!(terminal_resolutions.load(Ordering::Relaxed), 0);
        assert!(has_schema_diagnostic_at_field(&terminal_error, "output"));
    }
}

fn has_schema_diagnostic_at_field(error: &VerificationError, field: &str) -> bool {
    let VerificationError::Rejected { diagnostics } = error else {
        return false;
    };
    diagnostics.iter().any(|diagnostic| {
        diagnostic.code == GraphDiagnosticCode::SchemaSafety
            && serde_json::to_value(&diagnostic.path).is_ok_and(|path| {
                path.as_array().is_some_and(|segments| {
                    segments
                        .last()
                        .is_some_and(|segment| segment == &json!({"kind":"field","name":field}))
                })
            })
    })
}
use super::*;
