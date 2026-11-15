use super::*;
use openengine_cluster_protocol::{
    RunId, RunSuspendParams, RunVectorParams, RunVectorsParams, VectorId,
};

fn control_document(origin: &str) -> Value {
    let mut document = hosted_document(origin);
    document["extensions"]["run_suspension"] = json!({"kind":"openengine.run-suspension/v1"});
    document
}

#[tokio::test]
async fn hosted_request_profile_checks_its_graph_before_tokens_or_storage() {
    for marker in [None, Some("request_wait"), Some("request_ask")] {
        let root = temp_root();
        let target = RecordingTarget::spawn(move |request, origin, tokens| {
            if (request.method.as_str(), request.path.as_str()) == DISCOVERY {
                let mut document = control_document(origin);
                if let Some(marker) = marker {
                    document["extensions"][marker] = json!({"kind": if marker == "request_ask" {
                        "openengine.request-ask/v1"
                    } else { "openengine.request-wait/v1" }});
                }
                ("200 OK", document.to_string())
            } else {
                hosted_response(request, origin, tokens, true)
            }
        })
        .await;
        let (record, authority) = authenticated_authority(&root, &target.origin).await;
        let guide = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../docs/guides/ask-a-person.md"
        ));
        let document = |name| {
            let marker = format!("```json title=\"{name}\"\n");
            let text = guide
                .split_once(&marker)
                .assert_value()
                .1
                .split_once("```")
                .assert_value()
                .0;
            serde_json::from_str::<Value>(text).assert_value()
        };
        let mut profile = profile_request(lane_free_binding());
        profile.graph = serde_json::from_value(document("graph.json")).assert_value();
        profile.runtime = serde_json::from_value(document("runtime.json")).assert_value();
        let result = authority.profile_set(&record, profile).await;
        if marker == Some("request_ask") {
            result.assert_value();
            assert_eq!(
                routes(&target.stop()),
                [DISCOVERY, OAUTH_METADATA, TOKEN, SESSION, PROFILE_SET]
            );
        } else {
            assert!(
                result
                    .assert_error()
                    .to_string()
                    .contains("openengine.request-ask/v1")
            );
            assert_eq!(routes(&target.stop()), [DISCOVERY, OAUTH_METADATA]);
        }
    }
}

#[tokio::test]
async fn hosted_request_status_list_and_watch_accept_large_requests_with_bounded_reads() {
    use openengine_cluster_protocol::{
        RunListParams, RunLogsParams, RunStatusParams, RunWatchParams, MAX_REQUEST_RESPONSE_BYTES,
    };
    use zeroshot_engine::native_v2_cli::{CliSubscription, CliSubscriptionItem};
    for text_bytes in [1000, 70_000, 1024 * 1024 - 1024, MAX_REQUEST_RESPONSE_BYTES] {
        let root = temp_root();
        let status = json!({"phase":"suspended","reason":"request","node":"approval","createdAt":1000,
            "request":{"id":"question","text":"x".repeat(text_bytes),"signals":{"decision":["approved","rejected"]},
                "output":{"kind":"record","fields":{}},"expiresAt":2000},"metadata":{}});
        let returned = status.clone();
        let target = RecordingTarget::spawn(move |request, origin, tokens| {
            let run = json!({"runId":"question-run","title":"Approval","size":"medium","atCursor":"v2:1","status":returned});
            match (request.method.as_str(), request.path.as_str()) {
                DISCOVERY => ("200 OK", control_document(origin).to_string()),
                ("GET", "/native-v2/runs") => ("200 OK", json!({"runs":[run]}).to_string()),
                ("GET", "/native-v2/runs/question-run") => ("200 OK", run.to_string()),
                ("GET", "/native-v2/runs/question-run/watch") => ("200 OK", format!("{}\n{}\n",
                    json!({"type":"event","event":{"subscriptionId":"watch-1","runId":"question-run","title":"Approval",
                        "size":"medium","cursor":"v2:1","status":returned}}), json!({"type":"closed","reason":"done"}))),
                ("GET", "/native-v2/runs/question-run/logs") => ("200 OK", format!("{}\n",
                    json!({"type":"event","event":{"subscriptionId":"logs-1","runId":"question-run","cursor":"v2:2",
                        "timestamp":1000,"execution":"worker/1","record":{"level":"info","target":"agent","message":"x".repeat(70_000)}}}))),
                _ => hosted_response(request, origin, tokens, true),
            }
        }).await;
        let (record, authority) = authenticated_authority(&root, &target.origin).await;
        let observed = authority
            .hosted_run_status(
                &record,
                RunStatusParams {
                    run_id: RunId::new("question-run"),
                },
            )
            .await;
        let listed = authority.hosted_run_list(&record, RunListParams {}).await;
        let mut watch = authority
            .hosted_run_watch(
                &record,
                RunWatchParams {
                    run_id: RunId::new("question-run"),
                    from_cursor: None,
                },
            )
            .await
            .assert_value();
        let event = watch.next().await;
        if text_bytes == MAX_REQUEST_RESPONSE_BYTES {
            assert!(observed.assert_error().to_string().contains("too large"));
            assert!(listed.assert_error().to_string().contains("too large"));
            assert!(event.is_err());
        } else {
            assert_eq!(
                serde_json::to_value(observed.assert_value().status).assert_value(),
                status
            );
            assert_eq!(
                serde_json::to_value(&listed.assert_value().runs[0].status).assert_value(),
                status
            );
            let Some(CliSubscriptionItem::Event(event)) = event.assert_value() else {
                panic!("expected request watch event")
            };
            assert_eq!(serde_json::to_value(event.status).assert_value(), status);
            assert!(matches!(
                watch.next().await.assert_value(),
                Some(CliSubscriptionItem::Closed { .. })
            ));
            assert!(watch.next().await.assert_value().is_none());
        }
        if text_bytes == 70_000 {
            let mut logs = authority
                .hosted_run_logs(
                    &record,
                    RunLogsParams {
                        run_id: RunId::new("question-run"),
                        from_cursor: None,
                        execution: None,
                    },
                )
                .await
                .assert_value();
            assert!(
                logs.next().await.is_err(),
                "log frames retain the smaller bound"
            );
        }
        assert!(
            target
                .stop()
                .iter()
                .filter(|request| request.path.starts_with("/native-v2/runs"))
                .all(|request| request.authorization.as_deref() == Some("Bearer access-1"))
        );
    }
}

#[tokio::test]
async fn hosted_request_list_bounds_each_status_in_a_collection_of_large_requests() {
    let root = temp_root();
    let runs = (0..9).map(|index| json!({
        "runId":format!("request-{index}"),"title":"Approval","size":"medium","atCursor":"v2:1",
        "status":{"phase":"suspended","reason":"request","node":"approval","createdAt":1000,
            "request":{"id":format!("question-{index}"),"text":"x".repeat(750_000),
                "signals":{"decision":["approved","rejected"]},
                "output":{"kind":"record","fields":{}},"expiresAt":2000},"metadata":{}}
    })).collect::<Vec<_>>();
    let expected = json!({"runs":runs});
    let returned = expected.clone();
    assert!(
        returned.to_string().len() > 2 * openengine_cluster_protocol::MAX_REQUEST_RESPONSE_BYTES
    );
    let target = RecordingTarget::spawn(move |request, origin, tokens| {
        match (request.method.as_str(), request.path.as_str()) {
            DISCOVERY => ("200 OK", control_document(origin).to_string()),
            ("GET", "/native-v2/runs") => ("200 OK", returned.to_string()),
            _ => hosted_response(request, origin, tokens, true),
        }
    })
    .await;
    let (record, authority) = authenticated_authority(&root, &target.origin).await;
    let result = authority
        .hosted_run_list(&record, openengine_cluster_protocol::RunListParams {})
        .await
        .assert_value();
    assert_eq!(serde_json::to_value(result).assert_value(), expected);
    assert_eq!(
        routes(&target.stop()),
        [
            DISCOVERY,
            OAUTH_METADATA,
            TOKEN,
            SESSION,
            ("GET", "/native-v2/runs")
        ]
    );
}

#[tokio::test]
async fn hosted_request_answer_requires_the_marker_and_uses_the_resume_route() {
    for enabled in [false, true] {
        let root = temp_root();
        let target = RecordingTarget::spawn(move |request, origin, tokens| {
            if (request.method.as_str(), request.path.as_str()) == DISCOVERY {
                let mut document = control_document(origin);
                if enabled {
                    document["extensions"]["request_ask"] =
                        json!({"kind":"openengine.request-ask/v1"});
                }
                ("200 OK", document.to_string())
            } else if (request.method.as_str(), request.path.as_str())
                == ("POST", "/native-v2/runs/run-recoverable/resume")
            {
                (
                    "200 OK",
                    json!({"runId":"successor","resumedFrom":"run-recoverable"}).to_string(),
                )
            } else {
                hosted_response(request, origin, tokens, true)
            }
        })
        .await;
        let (record, authority) = authenticated_authority(&root, &target.origin).await;
        let from = json!({"kind":"answer","signals":{"decision":"approved"},"output":{"feedback":"Ready."}});
        let params = serde_json::from_value(
            json!({"runId":"run-recoverable","successorRunId":"successor","from":from}),
        )
        .assert_value();
        let result = authority.hosted_run_resume(&record, params).await;
        if enabled {
            assert_eq!(result.assert_value().run_id.as_str(), "successor");
        } else {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("openengine.request-ask/v1")
            );
        }
        let requests = target.stop();
        if enabled {
            let sent = requests.last().assert_value();
            assert_eq!(sent.path, "/native-v2/runs/run-recoverable/resume");
            assert_eq!(sent.authorization.as_deref(), Some("Bearer access-1"));
            assert_eq!(
                serde_json::from_str::<Value>(&sent.body).assert_value()["from"],
                from
            );
        } else {
            assert!(requests.iter().all(|request| request.path != "/token"
                && request.path != "/session"
                && request.body.is_empty()));
        }
    }
}

#[tokio::test]
async fn hosted_suspend_uses_the_advertised_authenticated_http_route() {
    let root = temp_root();
    let target = RecordingTarget::spawn(|request, origin, tokens| {
        match (request.method.as_str(), request.path.as_str()) {
            DISCOVERY => ("200 OK", control_document(origin).to_string()),
            ("POST", "/native-v2/runs/run-recoverable/suspend") => ("200 OK", json!({
                "runId":"run-recoverable", "title":"Suspend", "source":null, "size":"medium", "atCursor":"1",
                "status":{"phase":"running","activeExecutions":[],"suspendRequested":true}
            }).to_string()),
            _ => hosted_response(request, origin, tokens, true),
        }
    }).await;
    let (record, authority) = authenticated_authority(&root, &target.origin).await;
    let result = authority
        .hosted_run_suspend(
            &record,
            RunSuspendParams {
                run_id: RunId::new("run-recoverable"),
            },
        )
        .await
        .assert_value()
        .assert_value();
    assert!(matches!(
        result.status,
        openengine_cluster_protocol::HostedRunStatus::Target(
            openengine_cluster_protocol::RunStatus::Running {
                suspend_requested: true,
                ..
            }
        )
    ));
    let requests = target.stop();
    assert_eq!(
        routes(&requests),
        [
            DISCOVERY,
            OAUTH_METADATA,
            TOKEN,
            SESSION,
            ("POST", "/native-v2/runs/run-recoverable/suspend")
        ]
    );
    let request = requests.last().assert_value();
    assert_eq!(request.authorization.as_deref(), Some("Bearer access-1"));
    assert_eq!(
        serde_json::from_str::<Value>(&request.body).assert_value(),
        json!({"runId":"run-recoverable"})
    );
}

#[tokio::test]
async fn absent_hosted_control_routes_fall_back_without_resolving_access_tokens() {
    for operation in 0..3 {
        let root = temp_root();
        let target = RecordingTarget::spawn(|request, origin, tokens| {
            if (request.method.as_str(), request.path.as_str()) == DISCOVERY {
                let mut document = control_document(origin);
                document["extensions"]
                    .as_object_mut()
                    .assert_value()
                    .remove("hosted_run_controls");
                ("200 OK", document.to_string())
            } else {
                hosted_response(request, origin, tokens, true)
            }
        })
        .await;
        let (record, authority) = authenticated_authority(&root, &target.origin).await;
        let run_id = RunId::new("run-recoverable");
        let missing = match operation {
            0 => authority
                .hosted_run_suspend(&record, RunSuspendParams { run_id })
                .await
                .assert_value()
                .is_none(),
            1 => authority
                .hosted_run_vectors(
                    &record,
                    RunVectorsParams {
                        run_id,
                        after: None,
                        limit: None,
                    },
                )
                .await
                .assert_value()
                .is_none(),
            _ => authority
                .hosted_run_vector(
                    &record,
                    RunVectorParams {
                        run_id,
                        id: VectorId::new("entry").assert_value(),
                    },
                )
                .await
                .assert_value()
                .is_none(),
        };
        assert!(missing);
        let requests = target.stop();
        assert_eq!(routes(&requests), [DISCOVERY, OAUTH_METADATA]);
        assert!(requests.iter().all(|request| request.body.is_empty()));
    }
}

#[tokio::test]
async fn request_capability_and_missing_inbox_routes_fail_before_access_tokens() {
    use openengine_cluster_protocol::InboxListParams;
    for marker in [false, true] {
        let root = temp_root();
        let target = RecordingTarget::spawn(move |request, origin, tokens| {
            if (request.method.as_str(), request.path.as_str()) == DISCOVERY {
                let mut document = control_document(origin);
                if marker {
                    document["extensions"]["request_ask"] =
                        json!({"kind":"openengine.request-ask/v1"});
                }
                ("200 OK", document.to_string())
            } else {
                hosted_response(request, origin, tokens, true)
            }
        })
        .await;
        let (record, authority) = authenticated_authority(&root, &target.origin).await;
        let error = authority
            .hosted_inbox_list(&record, InboxListParams::default())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains(if marker {
                "inbox"
            } else {
                "openengine.request-ask/v1"
            }),
            "{error}"
        );
        let requests = target.stop();
        assert!(
            requests
                .iter()
                .all(
                    |request| (request.method.as_str(), request.path.as_str()) == DISCOVERY
                        || (request.method.as_str(), request.path.as_str()) == OAUTH_METADATA
                )
        );
    }
}
#[tokio::test]
async fn hosted_inbox_reads_advertised_routes_with_an_explicit_large_show_bound() {
    use openengine_cluster_protocol::{InboxListParams, RunStatusParams};
    let root = temp_root();
    let target=RecordingTarget::spawn(|request,origin,tokens| {
        match (request.method.as_str(),request.path.as_str()) {
            DISCOVERY=>{let mut document=control_document(origin);document["extensions"]["request_ask"]=json!({"kind":"openengine.request-ask/v1"});document["extensions"]["hosted_inbox"]=json!({"kind":"openengine.hosted-inbox/v1","route_templates":{"list":"/inbox/list","show":"/inbox/{run_id}"}});("200 OK",document.to_string())},
            ("POST","/inbox/list")=>("200 OK",json!({"requests":[{"runId":"question-run","node":"approval","createdAt":1000,"expiresAt":2000,"state":"pending"}],"nextAfter":"question-run"}).to_string()),
            ("POST","/inbox/question-run")=>("200 OK",json!({"runId":"question-run","node":"approval","createdAt":1000,"request":{"id":"question","text":"x".repeat(1024 * 1024 - 1024),"signals":{"decision":["approved","rejected"]},"output":{"kind":"record","fields":{}},"expiresAt":2000},"state":"pending"}).to_string()),
            _=>hosted_response(request,origin,tokens,true),
        }
    }).await;
    let (record, authority) = authenticated_authority(&root, &target.origin).await;
    let page = authority
        .hosted_inbox_list(
            &record,
            InboxListParams {
                after: Some(RunId::new("before")),
                limit: Some(100),
            },
        )
        .await
        .assert_value()
        .assert_value();
    assert_eq!(page.next_after, Some(RunId::new("question-run")));
    let show = authority
        .hosted_inbox_show(
            &record,
            RunStatusParams {
                run_id: RunId::new("question-run"),
            },
        )
        .await
        .assert_value()
        .assert_value();
    assert_eq!(show.request.text.len(), 1024 * 1024 - 1024);
    let requests = target.stop();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path.starts_with("/inbox/"))
            .count(),
        2
    );
    assert!(
        requests
            .iter()
            .filter(|request| request.path.starts_with("/inbox/"))
            .all(|request| request.authorization.as_deref() == Some("Bearer access-1"))
    );
}
