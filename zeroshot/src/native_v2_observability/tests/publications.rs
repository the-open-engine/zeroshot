use super::*;
use openengine_cluster_protocol::{RunSuspension, RunVector, RunVectorContent, UnixTimestampMillis};
use crate::native_v2_observability::history::validate_history_page;
use crate::v2_run_ledger::{PendingRequest, initial_cursor};

#[tokio::test]
async fn request_history_hides_preparation_and_keeps_attachments_in_the_authorized_inbox() {
    for committed in [false, true] {
        let (ledger, id) = ledger_run(&uuid::Uuid::now_v7().to_string()).await;
        let mut suspension: RunSuspension = serde_json::from_value(serde_json::json!({
            "reason":"request", "node":"worker", "createdAt":1,
            "request":{"id":"private-prepared-request", "text":"Private preparation text",
                "signals":{"decision":["approved","rejected"]},
                "output":{"kind":"record","fields":{}}, "expiresAt":1000}
        }))
        .assert_value();
        ledger
            .append(
                &id,
                vec![
                    RunEvent::RunStarted,
                    RunEvent::RequestPrepared {
                        pending: PendingRequest {
                            suspension: suspension.clone(),
                            paths: BTreeMap::from([(
                                openengine_cluster_protocol::FieldName::new("report")
                                    .assert_value(),
                                "private-selected-path".to_owned(),
                            )]),
                        },
                    },
                ],
            )
            .await
            .assert_value();
        let observations = NativeV2Observability::new(ledger.clone());
        let history = observations.history();
        let definition = history.definition(&id).await.assert_value();
        assert!(definition.suspension.is_none());
        let prepared = history.page(&id, None).await.assert_value();
        assert_eq!(prepared.events.len(), 2);
        assert_eq!(
            prepared.events[1]["event"],
            serde_json::json!({"kind":"request_prepared"})
        );
        assert_eq!(prepared.next_cursor.as_str(), "v2:2");
        validate_history_page(&prepared, &initial_cursor()).assert_value();
        assert!(
            crate::v2_run_ledger::inbox::show(
                ledger.as_ref(),
                &id,
                UnixTimestampMillis::new(2).assert_value()
            )
            .await
            .is_err()
        );

        if committed {
            let RunSuspension::Request {
                node,
                created_at,
                request,
            } = &mut suspension
            else {
                panic!("request")
            };
            request.artifacts = serde_json::from_value(serde_json::json!({
                "local":{"kind":"local","root":"/private/working","path":"report"},
                "managed":{"kind":"managed","target":"target","address":{"scope":{"kind":"run","runId":id},"path":"requests/private/report"}}
            })).assert_value();
            let vector = RunVector {
                id: request.id.clone(),
                run_id: id.clone(),
                created_at: *created_at,
                content: RunVectorContent::Request {
                    artifacts: request.artifacts.clone(),
                    node: node.clone(),
                    text: request.text.clone(),
                    signals: request.signals.clone(),
                    output: request.output.clone(),
                    expires_at: request.expires_at,
                },
            };
            ledger
                .append(
                    &id,
                    vec![
                        RunEvent::Vector { vector },
                        RunEvent::Suspended {
                            suspension: suspension.clone(),
                        },
                    ],
                )
                .await
                .assert_value();
            let inbox = crate::v2_run_ledger::inbox::show(
                ledger.as_ref(),
                &id,
                UnixTimestampMillis::new(2).assert_value(),
            )
            .await
            .assert_value();
            assert_eq!(inbox.request.artifacts, request_artifacts(&suspension));
            assert_eq!(inbox.request.artifacts.len(), 2);
            let definition =
                serde_json::to_value(history.definition(&id).await.assert_value()).assert_value();
            assert!(
                definition["suspension"]["request"]
                    .get("artifacts")
                    .is_none()
            );
            let status = serde_json::to_value(
                observations
                    .status(RunStatusParams::new(id.clone()))
                    .await
                    .assert_value(),
            )
            .assert_value();
            assert!(!status.to_string().contains("/private/working"));
        } else {
            ledger
                .append(
                    &id,
                    vec![RunEvent::Terminal {
                        result: TerminalResult::Failed {
                            reason: openengine_cluster_protocol::EnumLabel::new("runtime_failed")
                                .assert_value(),
                            context: None,
                        },
                    }],
                )
                .await
                .assert_value();
            assert!(
                history
                    .definition(&id)
                    .await
                    .assert_value()
                    .suspension
                    .is_none()
            );
            assert!(
                crate::v2_run_ledger::inbox::show(
                    ledger.as_ref(),
                    &id,
                    UnixTimestampMillis::new(2).assert_value()
                )
                .await
                .is_err()
            );
        }
        let page = history.page(&id, None).await.assert_value();
        validate_history_page(&page, &initial_cursor()).assert_value();
        assert_eq!(
            page.events[1]["event"],
            serde_json::json!({"kind":"request_prepared"})
        );
        assert!(
            !serde_json::to_string(&page)
                .assert_value()
                .contains("private-selected-path")
        );
        for event in &page.events {
            assert!(event["event"]["vector"].get("artifacts").is_none());
            assert!(
                event["event"]["suspension"]["request"]
                    .get("artifacts")
                    .is_none()
            );
        }
        let resumed = history
            .page(&id, Some(prepared.next_cursor.clone()))
            .await
            .assert_value();
        validate_history_page(&resumed, &prepared.next_cursor).assert_value();
        assert_eq!(resumed.events, page.events[2..]);
        assert_eq!(resumed.next_cursor, page.next_cursor);
        // Projection must not mutate the retained request or its authorized inbox.
        if committed {
            assert_eq!(
                ledger
                    .get(&id)
                    .await
                    .assert_value()
                    .assert_value()
                    .snapshot
                    .suspension,
                Some(suspension)
            );
        }
    }
}

fn request_artifacts(suspension: &RunSuspension) -> openengine_cluster_protocol::PublishedFiles {
    let RunSuspension::Request { request, .. } = suspension else {
        panic!("request")
    };
    request.artifacts.clone()
}
