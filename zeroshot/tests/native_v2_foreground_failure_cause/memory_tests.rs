use super::*;
use std::os::unix::fs::PermissionsExt;
use serde_json::json;
use sha2::{Digest, Sha256};

fn memory_runtime(fixture: &LocalFixture, enabled: bool, postmortem: bool) {
    let path = fixture.repository.parent().unwrap().join("runtime.json");
    let mut runtime: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    runtime["activeMemory"] = json!({"repository":enabled,"graph":enabled});
    runtime["globalAgents"] = json!({"postmortem":{"enabled":postmortem}});
    fs::write(path, serde_json::to_vec(&runtime).unwrap()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_memory_store_is_visible_with_postmortem_disabled_and_preserves_success() {
    for enabled in [true, false] {
        let fixture = LocalFixture::new().without_checkpoints();
        memory_runtime(&fixture, enabled, false);
        let memory = fixture
            .repository
            .parent()
            .unwrap()
            .join("state/active-memory");
        fs::create_dir_all(&memory).unwrap();
        fs::set_permissions(&memory, fs::Permissions::from_mode(0o700)).unwrap();
        let store = memory.join("lessons.sqlite3");
        fs::write(&store, b"this is not a SQLite database").unwrap();
        fs::set_permissions(&store, fs::Permissions::from_mode(0o600)).unwrap();
        let result = fixture.run("Memory diagnostics", "finish", false).await;
        assert_success(&result, "ordinary memory failure must not fail the graph");
        let stderr = String::from_utf8_lossy(&result.stderr);
        let receipt = json_lines(&result.stdout);
        let run_id = receipt[0]["runId"].as_str().unwrap();
        let logs = fixture.output(&["logs", run_id], "finish").await;
        assert_success(&logs, "read durable memory error");
        let logs = String::from_utf8_lossy(&logs.stdout);
        if enabled {
            for text in [
                store.to_str().unwrap(),
                "not a database",
                "recall and postmortem unavailable",
            ] {
                assert!(stderr.contains(text), "foreground lost {text}: {stderr}");
                assert!(logs.contains(text), "durable log lost {text}: {logs}");
            }
            assert_eq!(
                stderr.matches("recall and postmortem unavailable").count(),
                1
            );
            assert!(logs.contains("\"level\":\"error\""));
            let failed = fixture
                .run("Resume memory diagnostics", "fail", false)
                .await;
            assert!(!failed.status.success());
            let receipt = json_lines(&failed.stdout);
            let predecessor = receipt[0]["runId"].as_str().unwrap();
            let resumed = fixture
                .output(&["resume", predecessor, "--restart"], "finish")
                .await;
            assert_success(&resumed, "resume keeps memory failures nonfatal");
            let resumed_stderr = String::from_utf8_lossy(&resumed.stderr);
            assert!(
                resumed_stderr.contains(store.to_str().unwrap()),
                "{resumed_stderr}"
            );
            assert!(
                resumed_stderr.contains("not a database"),
                "{resumed_stderr}"
            );
        } else {
            assert!(!stderr.contains("Active Memory"), "{stderr}");
            assert!(
                !logs.contains("recall and postmortem unavailable"),
                "{logs}"
            );
        }
        assert_eq!(fs::read(&store).unwrap(), b"this is not a SQLite database");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_memory_records_report_safe_causes_without_exposing_stored_values() {
    let fixture = LocalFixture::new().without_checkpoints();
    memory_runtime(&fixture, true, false);
    let seeded = fixture.run("Initialize memory", "finish", false).await;
    assert_success(&seeded, "initialize healthy memory");
    let receipt = json_lines(&seeded.stdout);
    let ledger = rusqlite::Connection::open(
        fixture
            .run_storage(receipt[0]["runId"].as_str().unwrap())
            .join("runs.sqlite3"),
    )
    .unwrap();
    let stored: String = ledger
        .query_row("SELECT stored_json FROM v2_runs LIMIT 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let admitted: Value = serde_json::from_str(&stored).unwrap();
    let graph: openengine_cluster_protocol::CompiledGraphIr =
        serde_json::from_value(admitted["admitted"]["graph"].clone()).unwrap();
    let graph_hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&graph).unwrap()));
    drop(ledger);
    let store = fixture
        .repository
        .parent()
        .unwrap()
        .join("state/active-memory/lessons.sqlite3");
    // This is the fixture's declared provider credential. Neither a whole-record type error nor
    // an invalid scope tag may echo stored text, even if its value is unknown to a redactor.
    let private = "local-declared-key";
    for body in [json!(private), json!({"scope":{"kind":private}})] {
        let body = body.to_string();
        let database = rusqlite::Connection::open(&store).unwrap();
        database.execute(
            "INSERT OR REPLACE INTO active_memory_records(id,revision,deleted,kind,graph_hash,body) VALUES('malformed',1,0,'graph',?1,?2)",
            rusqlite::params![graph_hash, body],
        ).unwrap();
        drop(database);
        let result = fixture
            .run("Malformed memory diagnostics", "finish", false)
            .await;
        assert_success(&result, "malformed memory must preserve graph success");
        let receipt = json_lines(&result.stdout);
        let logs = fixture
            .output(&["logs", receipt[0]["runId"].as_str().unwrap()], "finish")
            .await;
        assert_success(&logs, "read malformed-record diagnostic");
        for text in [
            String::from_utf8_lossy(&result.stderr),
            String::from_utf8_lossy(&logs.stdout),
        ] {
            assert!(!text.contains(private), "diagnostic exposed stored value");
            for expected in [
                store.to_str().unwrap(),
                "record encoding failed",
                "Data JSON error",
                "line 1 column",
            ] {
                assert!(
                    text.contains(expected),
                    "diagnostic lost {expected}: {text}"
                );
            }
        }
        let status = fixture
            .json(&["status", receipt[0]["runId"].as_str().unwrap()], "finish")
            .await;
        assert_eq!(status["status"]["terminalResult"]["status"], "succeeded");
        let unchanged: String = rusqlite::Connection::open(&store)
            .unwrap()
            .query_row(
                "SELECT body FROM active_memory_records WHERE id='malformed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            unchanged, body,
            "diagnostics must leave the record unchanged"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn postmortem_provider_without_answer_reports_redacted_cause_and_one_summary() {
    let fixture = LocalFixture::new().without_checkpoints();
    memory_runtime(&fixture, true, true);
    let codex = fixture.repository.parent().unwrap().join("bin/codex");
    let original = fs::read_to_string(&codex).unwrap();
    let (_, rest) = original.split_once('\n').unwrap();
    fs::write(&codex, format!("#!/bin/sh\ncase \"${{ZEROSHOT_NODE_NAME-}}\" in active_memory_*) echo 'MODEL-CAUSE: unavailable account local-declared-key' >&2; exit 1;; esac\n{rest}")).unwrap();
    let result = fixture.run("Role diagnostics", "finish", false).await;
    assert_success(&result, "postmortem failure must preserve graph success");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("MODEL-CAUSE: unavailable account [REDACTED]"),
        "{stderr}"
    );
    assert!(!stderr.contains("local-declared-key"), "{stderr}");
    assert_eq!(
        stderr.matches("postmortem completed:").count(),
        1,
        "{stderr}"
    );
    let receipt = json_lines(&result.stdout);
    let run_id = receipt[0]["runId"].as_str().unwrap();
    let logs = fixture.output(&["logs", run_id], "finish").await;
    let logs = String::from_utf8_lossy(&logs.stdout);
    assert!(logs.contains("MODEL-CAUSE"), "{logs}");
    assert!(!logs.contains("local-declared-key"), "{logs}");
    for role in [
        "active_memory_graph",
        "active_memory_repository",
        "active_memory_graph_maintenance",
        "active_memory_repository_maintenance",
    ] {
        assert!(logs.contains(&format!("{role} failed:")), "{logs}");
    }
    let status = fixture.json(&["status", run_id], "finish").await;
    assert_eq!(status["status"]["terminalResult"]["status"], "succeeded");
}
