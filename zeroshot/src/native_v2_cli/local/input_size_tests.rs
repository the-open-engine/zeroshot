use super::*;

#[tokio::test]
async fn oversized_submission_is_rejected_before_local_run_storage_or_bootstrap() {
    for size in [3_000_000, 9_700_000] {
        let root = TestDirectory::new("submission-size");
        let backend = contract_backend(root.path());
        let run_id = RunId::new("01900000-0000-7000-8000-000000000162");
        let mut submission = contract_submission("oversized-submission");
        let mut graph = serde_json::to_value(&submission.graph).assert_value();
        let schema =
            json!({"kind":"record","fields":{"task":{"required":true,"type":{"kind":"string"}}}});
        graph["initialInput"] = schema.clone();
        graph["root"]["state"] = schema;
        submission.graph = serde_json::from_value(graph).assert_value();
        submission.initial_input = json!({"task":"x".repeat(size)});
        let input_bytes = serde_json::to_vec(&submission.initial_input)
            .assert_value()
            .len();
        let admitted = NativeV2Admission
            .admit(submission.clone())
            .await
            .assert_value();
        let submission_bytes = serde_json::to_vec(&admitted).assert_value().len();
        let error = backend
            .submit_prepared(
                prepared_local_run(run_id.clone(), submission, root.path().to_owned()),
                None,
            )
            .await
            .expect_err("oversized submission");
        let diagnostic = serde_json::to_value(error.diagnostic()).assert_value();
        assert_eq!(diagnostic["kind"], "invalid_request");
        assert_eq!(diagnostic["code"], "submission.too_large");
        for detail in [
            submission_bytes.to_string(),
            input_bytes.to_string(),
            "2097152".into(),
            "2 MiB".into(),
        ] {
            assert!(
                error.to_string().contains(&detail),
                "missing {detail}: {error}"
            );
        }
        assert!(!backend.run_storage(&run_id).assert_value().exists());
    }
}
