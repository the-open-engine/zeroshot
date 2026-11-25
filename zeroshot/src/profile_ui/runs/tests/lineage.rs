use super::*;
use openengine_cluster_protocol::{RunSuspension, RunVectorContent};
use crate::v2_run_ledger::vector_event;

async fn get(client: &reqwest::Client, server: &str, path: &str) -> Value {
    let response = client
        .get(format!("http://{server}/ui/api/runs{path}"))
        .send()
        .await
        .assert_value()
        .error_for_status()
        .assert_value();
    response.json().await.assert_value()
}

async fn recovery(fixture: &Fixture, part: &RunId, parent: Option<&RunId>, child: Option<&RunId>) {
    let detail = fixture
        .service
        .physical_detail(&fixture.id)
        .await
        .assert_value();
    let document = json!({
        "submission":{
            "submissionKey":"browser-history","graph":detail.graph,"runtime":detail.runtime,
            "initialInput":detail.initial_input,"title":detail.title
        },
        "workspace":fixture.root,"resumedFrom":parent,"successorRunId":child
    });
    std::fs::write(
        fixture
            .root
            .join("runs")
            .join(part.as_str())
            .join("workspace-recovery.json"),
        serde_json::to_vec(&document).assert_value(),
    )
    .assert_value();
}

pub(super) async fn successor(fixture: &Fixture, id: &RunId) -> SqliteRunLedger {
    let directory = fixture.root.join("runs").join(id.as_str());
    crate::execution::platform::create_private_directory(&directory).assert_value();
    let ledger = SqliteRunLedger::open(directory.join("runs.sqlite3")).assert_value();
    let source = fixture
        .ledger
        .get(&fixture.id)
        .await
        .assert_value()
        .assert_value();
    ledger
        .create_or_get(CreateRun {
            run_id: id.clone(),
            submission_key: source.submission_key,
            submission_digest: source.submission_digest,
            admitted: source.admitted,
        })
        .await
        .assert_value();
    ledger
}

pub(super) async fn suspended(fixture: &Fixture) {
    fixture
        .ledger
        .append(
            &fixture.id,
            vec![
                RunEvent::RunStarted,
                vector_event(&fixture.id, RunVectorContent::SuspendRequest {}).assert_value(),
                RunEvent::Suspended {
                    suspension: RunSuspension::Command {
                        node: NodeName::new("worker").assert_value(),
                        created_at: UnixTimestampMillis::new(15).assert_value(),
                    },
                },
            ],
        )
        .await
        .assert_value();
}

#[tokio::test]
async fn logical_detail_paging_and_follow_cross_a_confirmed_local_handoff() {
    let fixture = Fixture::new().await;
    suspended(&fixture).await;
    let root = fixture.id.as_str();
    let child = RunId::new(uuid::Uuid::now_v7().to_string());
    recovery(&fixture, &fixture.id, None, Some(&child)).await;
    let (address, task) = serve_local_ui(&fixture).await;
    let client = reqwest::Client::new();
    let waiting = get(&client, &address, &format!("/{root}")).await;
    assert_eq!(waiting["phase"], "suspended");
    assert_eq!(waiting["currentPartId"], root);
    let head = get(&client, &address, &format!("/{root}/history")).await;
    assert_eq!(head["finished"], false);
    assert_eq!(head["events"][1]["runCursor"], format!("{root}@v2:2"));
    let stream = client
        .get(format!("http://{address}/ui/api/runs/{root}/events"))
        .header("last-event-id", format!("{root}@v2:3"))
        .send()
        .await
        .assert_value();
    assert_eq!(stream.status(), StatusCode::OK);
    let ledger = successor(&fixture, &child).await;
    // The physical admission is still provisional until the host corroborates ownership.
    assert_eq!(
        get(&client, &address, &format!("/{root}")).await["currentPartId"],
        root
    );
    crate::v2_run_ledger::vectors::inherit(&fixture.ledger, &ledger, (&fixture.id, &child), None)
        .await
        .assert_value();
    ledger
        .append(&child, vec![RunEvent::RunStarted, live::terminal()])
        .await
        .assert_value();
    recovery(&fixture, &child, Some(&fixture.id), None).await;

    let body = tokio::time::timeout(Duration::from_secs(5), stream.text())
        .await
        .assert_value()
        .assert_value();
    assert!(
        body.contains(&format!("id: {}@v2:4", child.as_str())),
        "{body}"
    );
    assert!(body.contains("\"finished\":true"));
    let detail = get(&client, &address, &format!("/{}", child.as_str())).await;
    assert_eq!(detail["runId"], root);
    assert_eq!(detail["parts"], json!([root, child.as_str()]));
    assert_eq!(detail["phase"], "finished");
    let menu = get(&client, &address, "").await;
    assert_eq!(menu["runs"].as_array().assert_value().len(), 1);
    assert_eq!(menu["runs"][0]["currentPartId"], child.as_str());
    let replay = get(
        &client,
        &address,
        &format!("/{root}/history?after={}@v2:0", child.as_str()),
    )
    .await;
    assert_eq!(replay["partId"], child.as_str());
    let events = replay["events"].as_array().assert_value();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["event"]["vector"]["kind"], "continuation");
    let bookmark = events[0]["runCursor"].as_str().assert_value();
    let reconnected = get(
        &client,
        &address,
        &format!("/{root}/history?after={bookmark}"),
    )
    .await;
    assert_eq!(reconnected["events"][0]["event"]["kind"], "run_started");
    let exact = get(
        &client,
        &address,
        &format!("/{root}/history?partId={}", child.as_str()),
    )
    .await;
    assert_eq!(exact["events"].as_array().assert_value().len(), 4);
    assert_eq!(exact["nextCursor"], "v2:4");
    assert_eq!(exact["runId"], root);
    assert_eq!(exact["partId"], child.as_str());
    assert_eq!(exact["currentPartId"], child.as_str());
    let first = get(&client, &address, &format!("/{root}?partId={root}")).await;
    assert_eq!(first["parts"], json!([root, child.as_str()]));
    assert_eq!(first["partId"], root);
    assert_eq!(first["currentPartId"], child.as_str());
    assert_eq!(first["phase"], "suspended");
    let exact_stream = client
        .get(format!(
            "http://{address}/ui/api/runs/{root}/events?partId={root}"
        ))
        .send()
        .await
        .assert_value()
        .text()
        .await
        .assert_value();
    assert!(exact_stream.contains(&format!("\"currentPartId\":\"{}\"", child.as_str())));
    assert!(exact_stream.contains("id: v2:3"));
    let foreign = uuid::Uuid::now_v7();
    let denial = client
        .get(format!(
            "http://{address}/ui/api/runs/{root}/history?after={foreign}@v2:1"
        ))
        .send()
        .await
        .assert_value();
    assert_eq!(denial.status(), StatusCode::BAD_REQUEST);
    task.abort();
}

#[tokio::test]
async fn grouping_precedes_the_menu_limit_even_when_the_root_is_on_a_later_physical_page() {
    let fixture = Fixture::new().await;
    suspended(&fixture).await;
    let child = RunId::new(uuid::Uuid::now_v7().to_string());
    let _ledger = successor(&fixture, &child).await;
    recovery(&fixture, &fixture.id, None, Some(&child)).await;
    recovery(&fixture, &child, Some(&fixture.id), None).await;
    for _ in 0..RUN_HISTORY_LIST_PAGE_SIZE - 1 {
        let other = RunId::new(uuid::Uuid::now_v7().to_string());
        std::fs::create_dir(fixture.root.join("runs").join(other.as_str())).assert_value();
    }
    let (address, task) = serve_local_ui(&fixture).await;
    let menu = get(&reqwest::Client::new(), &address, "").await;
    let entries = menu["runs"].as_array().assert_value();
    assert_eq!(entries.len(), RUN_HISTORY_LIST_PAGE_SIZE);
    let last = entries.last().assert_value();
    assert_eq!(last["runId"], fixture.id.as_str());
    assert_eq!(last["currentPartId"], child.as_str());
    assert!(menu["nextCursor"].is_null());
    task.abort();
}

#[tokio::test]
async fn checkpoint_history_preserves_its_seed_and_never_elects_an_unclaimed_failed_part() {
    let fixture = Fixture::new().await;
    fixture.start(1).await;
    let failed = RunEvent::Terminal {
        result: TerminalResult::Failed {
            reason: openengine_cluster_protocol::EnumLabel::new("runtime_failed").assert_value(),
            context: None,
        },
    };
    fixture
        .ledger
        .append(
            &fixture.id,
            vec![live::completed(&fixture, 1), failed.clone()],
        )
        .await
        .assert_value();
    let root = &fixture.id;
    let original = fixture.ledger.get(root).await.assert_value().assert_value();
    let mut seed = crate::native_v2_supervisor::durable_history(&original.snapshot).assert_value();
    let failed_child = RunId::new(uuid::Uuid::now_v7().to_string());
    let rejected = successor(&fixture, &failed_child).await;
    crate::v2_run_ledger::vectors::inherit(&fixture.ledger, &rejected, (root, &failed_child), None)
        .await
        .assert_value();
    rejected
        .append(&failed_child, vec![RunEvent::RunStarted, failed])
        .await
        .assert_value();
    recovery(&fixture, &failed_child, Some(root), None).await;
    let child = RunId::new(uuid::Uuid::now_v7().to_string());
    let resumed = successor(&fixture, &child).await;
    let checkpoint =
        openengine_cluster_protocol::CheckpointId::new("selected-checkpoint").assert_value();
    crate::v2_run_ledger::vectors::inherit(
        &fixture.ledger,
        &resumed,
        (root, &child),
        Some(checkpoint),
    )
    .await
    .assert_value();
    resumed
        .append(
            &child,
            vec![
                RunEvent::PriorExecution {
                    execution: seed.remove(0),
                },
                RunEvent::RunStarted,
                live::terminal(),
            ],
        )
        .await
        .assert_value();
    recovery(&fixture, root, None, Some(&child)).await;
    recovery(&fixture, &child, Some(root), None).await;
    let (address, task) = serve_local_ui(&fixture).await;
    let client = reqwest::Client::new();
    let detail = get(&client, &address, &format!("/{}", root.as_str())).await;
    assert_eq!(detail["parts"], json!([root, child]));
    let replay = get(
        &client,
        &address,
        &format!("/{}/history?after={}@v2:0", root.as_str(), child.as_str()),
    )
    .await;
    assert_eq!(replay["partAfterCursor"], "v2:0");
    assert_eq!(replay["events"][0]["resumeReason"], "checkpoint");
    assert_eq!(
        replay["events"][0]["event"]["vector"]["checkpointId"],
        "selected-checkpoint"
    );
    assert_eq!(replay["imports"][0]["event"]["kind"], "prior_execution");
    assert_eq!(
        replay["imports"][0]["runCursor"],
        format!("{}@v2:2", child.as_str())
    );
    assert!(
        replay["events"]
            .as_array()
            .assert_value()
            .iter()
            .all(|entry| entry["event"]["kind"] != "prior_execution")
    );
    task.abort();
}

#[tokio::test]
async fn grouped_diagnostic_bookmarks_reconnect_to_their_owning_part() {
    let fixture = Fixture::new().await;
    suspended(&fixture).await;
    let root = &fixture.id;
    let child = RunId::new(uuid::Uuid::now_v7().to_string());
    let ledger = successor(&fixture, &child).await;
    let diagnostic = json!({"message":"Verifier failed here", "cursor":"v2:2"});
    let context = json!({"primary":diagnostic,"recorded":[diagnostic],"verifiers":[diagnostic]});
    ledger
        .append(
            &child,
            vec![
                RunEvent::RunStarted,
                RunEvent::SafeLog {
                    diagnostic: None,
                    execution: None,
                    timestamp: UnixTimestampMillis::new(3).assert_value(),
                    stream: SafeLogStream::Output,
                    line: SafeLogLine::new("child verifier transcript").assert_value(),
                },
                RunEvent::Terminal {
                    result: serde_json::from_value(
                        json!({"status":"failed","reason":"unhandled","context":context}),
                    )
                    .assert_value(),
                },
            ],
        )
        .await
        .assert_value();
    recovery(&fixture, root, None, Some(&child)).await;
    recovery(&fixture, &child, Some(root), None).await;
    let (address, task) = serve_local_ui(&fixture).await;
    let client = reqwest::Client::new();
    let detail = get(&client, &address, &format!("/{}", root.as_str())).await;
    let child_history = get(
        &client,
        &address,
        &format!("/{}/history?after={}@v2:0", root.as_str(), child.as_str()),
    )
    .await;
    for evidence in [
        &detail["terminal"]["context"],
        &child_history["events"][2]["event"]["result"]["context"],
    ] {
        for entry in [
            &evidence["primary"],
            &evidence["recorded"][0],
            &evidence["verifiers"][0],
        ] {
            assert_eq!(entry["cursor"], "v2:2");
            assert_eq!(entry["partId"], child.as_str());
            assert_eq!(entry["runCursor"], format!("{}@v2:2", child.as_str()));
        }
    }
    let cursor = detail["terminal"]["context"]["verifiers"][0]["runCursor"]
        .as_str()
        .assert_value();
    let resumed = get(
        &client,
        &address,
        &format!("/{}/history?after={cursor}", root.as_str()),
    )
    .await;
    assert_eq!(resumed["partId"], child.as_str());
    assert_eq!(resumed["events"][0]["event"]["kind"], "terminal");
    task.abort();
}
