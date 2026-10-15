use super::*;
use std::io::Write;

fn issuance(path: &std::path::Path, value: &str) {
    let mut file = crate::execution::platform::private_file(
        path,
        crate::execution::platform::FileAccess::CreateNew,
    )
    .unwrap();
    file.write_all(value.as_bytes()).unwrap();
}

#[tokio::test]
async fn native_merge_renews_a_rejected_file_credential_without_agent_credentials() {
    let repo = TempRepo::delivery();
    let private = tempfile::tempdir().unwrap();
    let token_path = private.path().join("token");
    issuance(&token_path, "test-token");
    let token_file = TrustedGitHubTokenFile::new(token_path.clone(), &repo.workspace).unwrap();
    let authority = Arc::new(FakeGitHub::new(
        repo.remote.clone(),
        Script::CredentialExpires,
    ));
    let adapter = retained_adapter(
        &repo,
        authority.clone(),
        DeliveryPollPolicy::new(50, Duration::from_millis(5)).unwrap(),
        DeliveryLineage::original("renewable-file"),
    );
    let adapter = Arc::new(
        (*adapter)
            .clone()
            .with_trusted_github_token_file(Some(token_file)),
    );
    let renew = async {
        while authority.inspections.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let next = private.path().join("next");
        issuance(&next, "refreshed-token");
        fs::rename(next, &token_path).unwrap();
    };
    let execution = run_with_adapter_token(
        DeliveryRunRequest {
            repo: &repo,
            attempts: 50,
            mode: DeliveryMode::MergeV3,
            run_id: "renewable-file",
            refresh: None,
        },
        adapter,
        PullRequestFeedback::Consider,
        None,
    );
    let (execution, ()) = tokio::join!(execution, renew);
    assert_delivery_signal(&execution.outcome, DELIVERY_MERGED_LABEL);
    assert!(
        execution
            .output
            .iter()
            .any(|event| event.text.contains("temporarily unavailable"))
    );
    for event in execution.output {
        for secret in [
            "test-token",
            "refreshed-token",
            token_path.to_str().unwrap(),
        ] {
            assert!(!event.text.contains(secret));
        }
    }
    assert!(authority.merge_requests.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn explicit_node_token_wins_and_invalid_values_do_not_use_file_fallback() {
    for token in ["test-token", "", "bad token"] {
        let repo = TempRepo::delivery();
        let private = tempfile::tempdir().unwrap();
        let token_file =
            TrustedGitHubTokenFile::new(private.path().join("absent"), &repo.workspace).unwrap();
        let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::NoCi));
        let poll = DeliveryPollPolicy::new(5, Duration::ZERO).unwrap();
        let adapter = retained_adapter(
            &repo,
            authority.clone(),
            poll,
            DeliveryLineage::original("explicit-file"),
        );
        let adapter = Arc::new(
            (*adapter)
                .clone()
                .with_trusted_github_token_file(Some(token_file)),
        );
        let execution = run_with_adapter_token(
            DeliveryRunRequest {
                repo: &repo,
                attempts: 5,
                mode: DeliveryMode::MergeV3,
                run_id: "explicit-file",
                refresh: None,
            },
            adapter,
            PullRequestFeedback::Consider,
            Some(token),
        )
        .await;
        if token == "test-token" {
            assert_delivery_signal(&execution.outcome, DELIVERY_MERGED_LABEL);
        } else {
            assert_eq!(execution.outcome, WorkerOutcome::authentication_refusal());
            assert!(!authority.pushed.load(Ordering::SeqCst));
        }
    }
}

#[tokio::test]
async fn unavailable_initial_token_file_waits_under_native_cancellation_before_git_effects() {
    let repo = TempRepo::delivery();
    let private = tempfile::tempdir().unwrap();
    let file = TrustedGitHubTokenFile::new(private.path().join("not-issued-yet"), &repo.workspace)
        .unwrap();
    let authority = Arc::new(FakeGitHub::new(repo.remote.clone(), Script::NoCi));
    let adapter = retained_adapter(
        &repo,
        authority.clone(),
        DeliveryPollPolicy::default(),
        DeliveryLineage::original("cancel-file"),
    );
    let adapter = Arc::new(
        (*adapter)
            .clone()
            .with_trusted_github_token_file(Some(file)),
    );
    let mut handle = start_with_adapter_token(
        DeliveryRunRequest {
            repo: &repo,
            attempts: 1,
            mode: DeliveryMode::MergeV3,
            run_id: "cancel-file",
            refresh: None,
        },
        adapter,
        PullRequestFeedback::Consider,
        None,
    )
    .await;
    let mut events = handle.take_initial_output().unwrap();
    let output = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap();
    match output {
        crate::native_v2_runner::DurableNodeEvent::Output { output, .. } => assert!(
            output
                .text
                .contains("trusted GitHub credential unavailable")
        ),
        other => panic!("unexpected token-file wait event: {other:?}"),
    }
    handle.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), handle.completion())
            .await
            .unwrap(),
        Err(NodeRunnerError::Cancelled)
    ));
    assert_eq!(authority.delivery_reads.load(Ordering::SeqCst), 0);
    assert!(!authority.pushed.load(Ordering::SeqCst));
}
