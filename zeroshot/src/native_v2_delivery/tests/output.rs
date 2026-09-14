use super::*;

#[tokio::test]
async fn large_utf8_ci_feedback_reaches_the_graph_and_live_output() {
    let repo = TempRepo::delivery();
    let authority = Arc::new(FakeGitHub::new(
        repo.remote.clone(),
        Script::LargeCiDiagnostic,
    ));
    let result = run_delivery_execution(
        DeliveryRunRequest {
            repo: &repo,
            attempts: 2,
            mode: DeliveryMode::Merge,
            run_id: "large-diagnostics",
            refresh: None,
        },
        authority.clone(),
    )
    .await;
    assert_delivery_signal(&result.outcome, DELIVERY_CI_FAILED_LABEL);
    let diagnostic = outcome_diagnostic(&result.outcome);
    assert_eq!(
        diagnostic,
        "failed check: build\n".to_owned() + &"λ🦀".repeat(20_000)
    );
    assert!(
        result
            .output
            .iter()
            .all(|line| line.text.len() <= crate::native_v2_runner::MAX_LIVE_OUTPUT_BYTES)
    );
    let emitted = result
        .output
        .iter()
        .map(|line| line.text.as_str())
        .collect::<String>();
    assert!(emitted.contains(diagnostic));
    assert_eq!(authority.merge_requests.load(Ordering::SeqCst), 0);
}
