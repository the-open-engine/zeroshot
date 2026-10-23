use super::*;
use crate::native_v2_contract::TokenUsageDelta;
use crate::v2_run_ledger::RunEvent;
use openengine_cluster_protocol::{EnumLabel, TokenCount};

#[tokio::test]
async fn local_observer_preserves_durable_primary_result_after_postmortem_process_loss() {
    for failed in [false, true] {
        let root = TestDirectory::new("primary-restart");
        let storage = root.child("state");
        crate::execution::platform::create_private_directory(&storage).assert_value();
        let paths = PortableControllerPaths::new(storage);
        let run_id = RunId::new("run-portable-primary");
        let mut submitted = submission("portable-primary");
        let primary = if failed {
            submitted.graph.root = serde_json::from_value(serde_json::json!({
                "kind":"fail","name":"done","reason":"work_rejected"
            }))
            .assert_value();
            TerminalResult::Failed {
                reason: EnumLabel::new("work_rejected").assert_value(),
                context: None,
            }
        } else {
            TerminalResult::Succeeded {
                output: Value::Null,
            }
        };
        let admitted = NativeV2Admission
            .admit_with_policy(submitted.clone(), DeliveryPolicy::Optional)
            .await
            .assert_value();
        let ledger = SqliteRunLedger::open(paths.ledger()).assert_value();
        ledger
            .create_or_get(CreateRun {
                run_id: run_id.clone(),
                submission_key: submitted.submission_key.clone(),
                submission_digest: Sha256Digest::new(format!(
                    "{:x}",
                    Sha256::digest(serde_json::to_vec(&submitted).assert_value())
                ))
                .assert_value(),
                admitted,
            })
            .await
            .assert_value();
        ledger
            .append(
                &run_id,
                vec![
                    RunEvent::RunStarted,
                    RunEvent::PrimaryGraphCompleted {
                        result: primary.clone(),
                        auxiliary_pending: true,
                    },
                    RunEvent::RunTokenUsageObserved {
                        usage: Some(TokenUsageDelta {
                            input_tokens: TokenCount::new(17).assert_value(),
                            output_tokens: TokenCount::new(23).assert_value(),
                            cache_read_input_tokens: None,
                            cache_creation_input_tokens: None,
                        }),
                    },
                ],
            )
            .await
            .assert_value();
        drop(ledger);
        let observer = PortableRunController::open_observer(paths.clone(), run_id.clone())
            .await
            .assert_value();
        let status = observer
            .inner
            .status(RunStatusParams {
                run_id: run_id.clone(),
            })
            .await
            .assert_value();
        let RunStatus::Finished {
            terminal_result,
            metadata,
        } = status.status
        else {
            panic!("lost optional work must finish after cleanup");
        };
        assert_eq!(terminal_result, primary);
        let usage = metadata.token_usage.assert_value();
        assert_eq!(usage.input_tokens.get(), 17);
        assert_eq!(usage.output_tokens.get(), 23);
        assert!(!usage.complete);
        drop(observer);
        let again = PortableRunController::open_observer(paths, run_id.clone())
            .await
            .assert_value();
        let replay = again
            .inner
            .status(RunStatusParams { run_id })
            .await
            .assert_value();
        assert_eq!(replay.at_cursor, status.at_cursor);
    }
}
