use crate::fixture::{Fixture, success};
use openengine_cluster_protocol::RunId;
use serde_json::{Value, json};
use std::sync::Arc;
use zeroshot_engine::native_v2_observability::NativeV2Observability;
use zeroshot_engine::v2_run_ledger::sqlite::SqliteRunLedger;

#[test]
fn script_verifier_signals_route_and_persist_without_model_calls() {
    for label in ["ready", "logged_out"] {
        let fixture = Fixture::new();
        let node = std::process::Command::new("node")
            .args(["-p", "process.execPath"])
            .output()
            .unwrap();
        success(&node);
        let node = String::from_utf8(node.stdout).unwrap();
        let empty = json!({"kind":"record","fields":{}});
        let diagnostic = json!({"kind":"record","fields":{
            "reason":{"type":{"kind":"string"},"required":true}
        }});
        let step = |name| {
            json!({
                "kind":"step", "name":name, "worker":"test.route@1",
                "input":{"kind":"null"}, "output":{"kind":"string"},
                "inputBindings":[], "writeBindings":[], "attempts":1
            })
        };
        fixture.write(
            "graph.json",
            &json!({
                "profile":"openengine.graph.full/v1", "initialInput":empty,
                "policy":{"policy":"policy.native-v2@1","default":"deny"},
                "root":{"kind":"seq","name":"root","state":empty,"promotedStatePaths":[],
                    "children":[
                        {"kind":"verifier","name":"probe","worker":"test.probe@1",
                            "input":{"kind":"null"},"output":{"kind":"null"},
                            "inputBindings":[],"writeBindings":[],"attempts":1,
                            "signals":{"status":["ready","logged_out"]},"diagnostic":diagnostic},
                        {"kind":"choice","name":"route","state":empty,"promotedStatePaths":[],
                            "branches":[{"when":{"kind":"in",
                                "value":{"source":"signal","name":"probe","field":"status"},
                                "labels":["ready"]},"node":step("ready")}],
                            "otherwise":{"kind":"seq","name":"reject","state":empty,
                                "promotedStatePaths":[],"children":[step("logged_out"),
                                    {"kind":"fail","name":"stop","reason":"rejected"}]}},
                        {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
                    ]}
            }),
        );
        fixture.write("input.json", &json!({}));
        let probe = format!(
            "const fs = require('node:fs'); JSON.parse(fs.readFileSync(0, 'utf8')); \
             if (process.env.ZEROSHOT_NODE_KIND !== 'verifier') process.exit(9); \
             console.log(JSON.stringify({{output:null,signals:{{status:{label:?}}},\
             diagnostic:{{reason:'Session observation: {label}; token=' + process.env.OPENAI_API_KEY}}}}));"
        );
        let mut bindings = serde_json::Map::new();
        bindings.insert(
            "probe".to_owned(),
            json!({"kind":"script","command":[node.trim(),"-e",probe],
                "connections":{"openai":["OPENAI_API_KEY"]}}),
        );
        for branch in ["ready", "logged_out"] {
            let source = format!(
                "const fs = require('node:fs'); JSON.parse(fs.readFileSync(0, 'utf8')); \
                 fs.writeFileSync('route.txt', {branch:?}); console.log(JSON.stringify({branch:?}));"
            );
            bindings.insert(
                branch.to_owned(),
                json!({"kind":"script","command":[node.trim(),"-e",source]}),
            );
        }
        fixture.write(
            "runtime.json",
            &json!({
                "harness":"codex","provider":"openai","size":"host","workspaceCheckpoints":false,
                "activeMemory":{"repository":false,"graph":false},
                "globalAgents":{"postmortem":{"enabled":false}},"nodes":bindings
            }),
        );
        success(
            &fixture
                .run_command("finish", false)
                .arg("--validate-only")
                .output()
                .unwrap(),
        );
        let result = fixture.run("finish", false);
        if label == "ready" {
            success(&result);
        } else {
            assert_eq!(result.status.code(), Some(1));
        }
        assert_eq!(
            std::fs::read_to_string(fixture.workspace.join("route.txt")).unwrap(),
            label
        );
        assert!(
            !fixture.workspace.join("mutation.txt").exists(),
            "agent harness was invoked"
        );
        let receipt: Value =
            serde_json::from_slice(result.stdout.split(|byte| *byte == b'\n').next().unwrap())
                .unwrap();
        let id = receipt["runId"].as_str().unwrap();
        let status = fixture.json(&["status", id]);
        let terminal = &status["status"]["terminalResult"];
        assert_eq!(
            terminal["status"],
            if label == "ready" {
                "succeeded"
            } else {
                "failed"
            }
        );
        let logs = fixture.command(&["logs", id]).output().unwrap();
        success(&logs);
        let logs = String::from_utf8(logs.stdout).unwrap();
        let reports = logs
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|record| {
                record["record"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("Session observation:"))
            })
            .collect::<Vec<_>>();
        assert_eq!(reports.len(), 1, "{logs}");
        let message = reports[0]["record"]["message"].as_str().unwrap();
        let report: Value = serde_json::from_str(message).unwrap();
        assert_eq!(report["signals"]["status"], label);
        assert_eq!(
            report["diagnostic"]["reason"],
            format!("Session observation: {label}; token=[REDACTED]")
        );
        assert!(!logs.contains("fixture-key"));
        if label == "logged_out" {
            assert_eq!(terminal["reason"], "rejected");
            let context = &terminal["context"];
            assert_eq!(context["verifiers"].as_array().unwrap().len(), 1);
            let evidence = &context["verifiers"][0];
            assert_eq!(evidence["message"], message);
            assert_eq!(evidence["execution"]["node"], "probe");
            assert_eq!(evidence["execution"]["execution"], reports[0]["execution"]);
            assert_eq!(evidence["cursor"], reports[0]["cursor"]);
            assert!(context["primary"].is_null());
            assert!(context["recorded"].as_array().unwrap().is_empty());
        }
        let ledger = SqliteRunLedger::open_read_only(
            fixture.root.path(&format!("state/runs/{id}/runs.sqlite3")),
        )
        .unwrap();
        let history = NativeV2Observability::new(Arc::new(ledger)).history();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let events = runtime.block_on(async {
            let mut after = None;
            let mut events = Vec::new();
            loop {
                let page = history.page(&RunId::new(id), after).await.unwrap();
                events.extend(page.events.into_iter().map(|event| event["event"].clone()));
                if page.complete {
                    break;
                }
                after = Some(page.next_cursor);
            }
            events
        });
        let completions = events
            .iter()
            .filter(|event| event["kind"] == "node_completed")
            .collect::<Vec<_>>();
        assert_eq!(completions.len(), 2, "{events:?}");
        assert_eq!(completions[0]["completion"]["reference"]["node"], "probe");
        assert_eq!(
            completions[0]["completion"]["outcome"]["status"],
            "verifier"
        );
        assert_eq!(
            completions[0]["completion"]["outcome"]["signals"]["status"],
            label
        );
        assert_eq!(completions[1]["completion"]["reference"]["node"], label);
        assert_eq!(completions[1]["completion"]["outcome"]["output"], label);
        let report_events = events
            .iter()
            .filter(|event| event["kind"] == "safe_log" && event["diagnostic"] == "verifier")
            .collect::<Vec<_>>();
        assert_eq!(report_events.len(), 1);
        assert_eq!(report_events[0]["line"], message);
        assert_eq!(
            report_events[0]["execution"],
            completions[0]["completion"]["reference"]["execution"]
        );
        let usage = events
            .iter()
            .filter(|event| event["kind"] == "token_usage_observed")
            .collect::<Vec<_>>();
        assert_eq!(usage.len(), 2);
        for event in usage {
            assert_eq!(
                event["usage"],
                json!({"inputTokens":0,"outputTokens":0,"cacheReadInputTokens":0,"cacheCreationInputTokens":0})
            );
        }
        let timings = events
            .iter()
            .filter_map(|event| event["line"].as_str())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|value| value["event"] == "script_timing")
            .collect::<Vec<_>>();
        assert_eq!(timings.len(), 2);
        assert!(timings.iter().all(|timing| timing["succeeded"] == true));
    }
}
