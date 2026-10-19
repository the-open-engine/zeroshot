use super::*;
use crate::native_v2_delivery::tests::github_fixture::{FakeGitHub, Script};
use openengine_cluster_testkit::assertions::AssertValue;

fn adapter() -> NativeV2DeliveryAdapter {
    NativeV2DeliveryAdapter::new(
        NativeV2DeliveryConfig {
            delivery_run_id: RunId::new("cleanup-publication"),
            adopt_existing_delivery: false,
            git_identity: None,
            workspace: PathBuf::new(),
            git_program: "/usr/bin/git".into(),
            target: DeliveryTarget::new("acme/project", "main", "a".repeat(40)).assert_value(),
            poll: DeliveryPollPolicy::new(1, Duration::ZERO).assert_value(),
        },
        Arc::new(FakeGitHub::new(PathBuf::new(), Script::NoCi)),
    )
}

#[tokio::test]
async fn unconfirmed_delivery_cleanup_keeps_its_classification_and_never_waits_for_a_stalled_consumer()
 {
    use crate::native_v2_runner::{DurableNodeEvent, test_support::control};
    for stalled in [false, true] {
        let (_cancel, control, mut durable) = control();
        if stalled {
            control
                .emit(LiveOutput::new(LiveOutputStream::Output, "buffer full").assert_value())
                .await
                .assert_value();
        }
        let credentials = DeliveryCredentials {
            environment: None,
            token_file: None,
            token: "private-cleanup-token".to_owned(),
            git: None,
            forge_field: Some(GITHUB_TOKEN_ENV),
            github_git_token: String::new(),
            token_from_environment: false,
        };
        let stop = DeliveryStop::Explained {
            result: Box::new(Err(NodeRunnerError::CleanupUnconfirmed)),
            diagnostic: "original cleanup cause private-cleanup-token".to_owned(),
        };
        tokio::time::timeout(
            Duration::from_secs(1),
            adapter().report_stop(
                &stop,
                DeliveryDiagnostic {
                    credentials: &credentials,
                    control: &control,
                    repair_supported: false,
                },
            ),
        )
        .await
        .assert_value();
        assert_eq!(stop.result(), Err(NodeRunnerError::CleanupUnconfirmed));
        drop(control);
        let mut outputs = Vec::new();
        while let Ok(event) = durable.recv().await {
            let DurableNodeEvent::Output { output, .. } = event else {
                panic!("no token usage was lost");
            };
            assert!(!output.text.contains("private-cleanup-token"));
            outputs.push(output);
        }
        let last = outputs.last().assert_value();
        if stalled {
            assert_eq!(last.diagnostic, Some(FailureDiagnosticKind::Incomplete));
        } else {
            assert_eq!(last.diagnostic, Some(FailureDiagnosticKind::Failure));
            assert!(last.text.contains("original cleanup cause [REDACTED]"));
        }
    }
}
