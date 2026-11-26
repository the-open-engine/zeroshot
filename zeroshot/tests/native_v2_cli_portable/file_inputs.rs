//! File admission through the shipped CLI, controller handoff and local recovery.
use super::{Fixture, success};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn receipt(output: &std::process::Output) -> String {
    success(output);
    serde_json::from_slice::<Value>(&output.stdout).unwrap()["currentPartId"]
        .as_str()
        .unwrap()
        .into()
}
fn settled(fixture: &Fixture, id: &str, phase: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let status = fixture.json(&["status", id]);
        if status["status"]["phase"] == phase {
            return status;
        }
        assert_ne!(status["status"]["phase"], "finished", "{status}");
        assert!(Instant::now() < deadline, "{status}");
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn await_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !path.exists() {
        assert!(Instant::now() < deadline, "{}", path.display());
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn configure(fixture: &Fixture, request: bool) {
    let state = json!({"kind":"record","fields":{
        "text":{"required":true,"type":{"kind":"string"}},
        "left":{"required":true,"type":{"kind":"filepath"}},
        "right":{"required":true,"type":{"kind":"filepath"}},
        "folder":{"required":false,"type":{"kind":"filepath"}},
        "reply":{"required":false,"type":{"kind":"filepath"}}
    }});
    let state = json!({"kind":"record","fields":{"payload":{"required":true,"type":state}}});
    let mut graph: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
    let mut worker = graph["root"]["children"][0].clone();
    worker.as_object_mut().unwrap().remove("instructions");
    worker["input"] = state.clone();
    worker["inputBindings"] = json!([
        {"target":["payload"],"value":{"source":"state","path":["payload"]}}
    ]);
    graph["initialInput"] = state.clone();
    graph["root"]["state"] = state.clone();
    graph["root"]["children"][1]["state"] = state;
    graph["root"]["children"][0] = worker;
    let mut nodes = graph["root"]["children"].as_array().unwrap().clone();
    if request {
        nodes.insert(0, json!({"kind":"verifier","name":"approval","worker":"builtin.request.ask@1",
            "input":{"kind":"record","fields":{"text":{"required":true,"type":{"kind":"string"}}}},
            "output":{"kind":"record","fields":{"reply":{"required":true,"type":{"kind":"filepath"}}}},
            "signals":{},"diagnostic":{"kind":"record","fields":{"message":{"required":true,"type":{"kind":"string"}}}},
            "attempts":1,"inputBindings":[{"target":["text"],"value":{"source":"state","path":["payload","text"]}}],
            "writeBindings":[{"target":["payload","reply"],"value":{"node":"approval","channel":"out","path":["reply"]}}]}));
    }
    graph["root"]["children"] = json!(nodes);
    fixture.write("graph.json", &graph);
    let node = std::process::Command::new("node")
        .args(["-p", "process.execPath"])
        .output()
        .unwrap();
    success(&node);
    let node = String::from_utf8(node.stdout).unwrap();
    let output = serde_json::to_string(&fixture.root.path("observed.json")).unwrap();
    let ready = serde_json::to_string(&fixture.root.path("ready")).unwrap();
    let release = serde_json::to_string(&fixture.root.path("release")).unwrap();
    let fail = serde_json::to_string(&fixture.root.path("fail-once")).unwrap();
    let script = format!(
        r#"
        const fs=require('node:fs'); const v=JSON.parse(fs.readFileSync(0,'utf8')).payload;
        if(!process.env.ZEROSHOT_INPUTS || !v.left.startsWith(process.env.ZEROSHOT_INPUTS)) process.exit(41);
        fs.writeFileSync({ready},'ready');
        const timer=setInterval(()=>{{ if(!fs.existsSync({release})) return; clearInterval(timer);
          const result={{paths:v,left:fs.readFileSync(v.left,'utf8'),right:fs.readFileSync(v.right,'utf8')}};
          if(v.folder) result.folder=fs.readFileSync(require('node:path').join(v.folder,'data.csv'),'utf8');
          if(v.reply) result.reply=fs.readFileSync(v.reply,'utf8');
          fs.writeFileSync({output},JSON.stringify(result)); fs.writeFileSync('live-output','worker output');
          if(fs.existsSync({fail})) {{ fs.unlinkSync({fail}); process.exit(1); }}
          console.log('null');
        }},20);
    "#
    );
    let mut bindings = json!({"worker":{"kind":"script","command":[node.trim(),"-e",script]}});
    if request {
        bindings["approval"] = json!({"kind":"request"});
    }
    fixture.write(
        "runtime.json",
        &json!({"harness":"codex","provider":"openai","size":"host",
        "workspaceCheckpoints":true,"activeMemory":{"repository":false,"graph":false},
        "globalAgents":{"postmortem":{"enabled":false}},"nodes":bindings}),
    );
}
fn observed(fixture: &Fixture) -> Value {
    serde_json::from_slice(&fs::read(fixture.root.path("observed.json")).unwrap()).unwrap()
}

#[test]
fn parent_relative_paths_work_for_submission_directories_and_answers() {
    let fixture = Fixture::new();
    configure(&fixture, true);
    fs::create_dir(fixture.root.path("dataset")).unwrap();
    fs::write(fixture.root.path("dataset/data.csv"), "accepted input").unwrap();
    fixture.write(
        "input.json",
        &json!({"payload":{
            "text":"Supply a file", "left":"../dataset/data.csv",
            "right":"../dataset/data.csv", "folder":"../dataset"
        }}),
    );
    let id = receipt(
        &fixture
            .run_command("finish", true)
            .arg("--workspace")
            .output()
            .unwrap(),
    );
    settled(&fixture, &id, "suspended");
    fs::write(fixture.root.path("dataset/data.csv"), "changed original").unwrap();
    fs::write(fixture.root.path("answer.csv"), "accepted answer").unwrap();
    fixture.write(
        "answer.json",
        &json!({"signals":{},"output":{"reply":"../answer.csv"}}),
    );
    let part = receipt(
        &fixture
            .command(&["answer", &id, "--input"])
            .arg(fixture.root.path("answer.json"))
            .output()
            .unwrap(),
    );
    await_file(&fixture.root.path("ready"));
    fs::write(fixture.root.path("answer.csv"), "changed answer original").unwrap();
    fs::write(fixture.root.path("release"), "release").unwrap();
    let status = settled(&fixture, &part, "finished");
    assert_eq!(
        status["status"]["terminalResult"]["status"], "succeeded",
        "{status}"
    );
    let result = observed(&fixture);
    for field in ["left", "right", "folder"] {
        assert_eq!(result[field], "accepted input");
    }
    assert_eq!(result["reply"], "accepted answer");
    for field in ["left", "right", "folder", "reply"] {
        assert!(!Path::new(result["paths"][field].as_str().unwrap()).exists());
    }
    assert_eq!(
        fs::read(fixture.root.path("dataset/data.csv")).unwrap(),
        b"changed original"
    );
    assert_eq!(
        fs::read(fixture.root.path("answer.csv")).unwrap(),
        b"changed answer original"
    );
}

#[cfg(unix)]
#[test]
fn successful_cleanup_releases_input_directories_without_read_permissions() {
    for mode in [0o000, 0o300] {
        let fixture = Fixture::new();
        configure(&fixture, false);
        fs::create_dir(fixture.root.path("dataset")).unwrap();
        fs::write(fixture.root.path("dataset/data.csv"), "accepted input").unwrap();
        fixture.write(
            "input.json",
            &json!({"payload":{
                "text":"Read files", "left":fixture.root.path("dataset/data.csv"),
                "right":fixture.root.path("dataset/data.csv"), "folder":fixture.root.path("dataset")
            }}),
        );
        let mut runtime: Value =
            serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
        let script = runtime["nodes"]["worker"]["command"][2].as_str().unwrap();
        runtime["nodes"]["worker"]["command"][2] = json!(script.replace("console.log('null');", &format!(
            "fs.chmodSync(v.folder, {mode}); fs.chmodSync(require('node:path').dirname(v.folder), {mode}); console.log('null');"
        )));
        fixture.write("runtime.json", &runtime);
        fs::write(fixture.root.path("release"), "release").unwrap();
        let id = receipt(
            &fixture
                .run_command("finish", true)
                .arg("--workspace")
                .output()
                .unwrap(),
        );
        let status = settled(&fixture, &id, "finished");
        assert_eq!(
            status["status"]["terminalResult"]["status"], "succeeded",
            "mode {mode:o}: {status}"
        );
        let result = observed(&fixture);
        assert!(!Path::new(result["paths"]["folder"].as_str().unwrap()).exists());
        assert_eq!(
            fs::read(fixture.root.path("dataset/data.csv")).unwrap(),
            b"accepted input"
        );
        assert_eq!(
            fs::read(fixture.workspace.join("live-output")).unwrap(),
            b"worker output"
        );
        assert_eq!(
            fs::read_dir(fixture.root.path("state/files/objects"))
                .unwrap()
                .count(),
            0
        );
    }
}

#[cfg(unix)]
#[test]
fn restart_prepares_authority_before_replacing_unreadable_input_directories() {
    use std::os::unix::fs::PermissionsExt;
    let mut failures = Vec::new();
    for mode in [0o700, 0o000, 0o300] {
        let fixture = Fixture::new();
        configure(&fixture, false);
        let source = fixture.root.path("dataset");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("data.csv"), "accepted input").unwrap();
        fixture.write(
            "input.json",
            &json!({"payload":{
                "text":"Read files", "left":source.join("data.csv"),
                "right":source.join("data.csv"), "folder":source
            }}),
        );
        let mut runtime: Value =
            serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
        runtime["workspaceCheckpoints"] = json!(false);
        fixture.write("runtime.json", &runtime);
        fs::write(fixture.root.path("release"), "release").unwrap();
        fs::write(fixture.root.path("fail-once"), "fail").unwrap();
        let id = receipt(
            &fixture
                .run_command("finish", true)
                .arg("--workspace")
                .output()
                .unwrap(),
        );
        let failed = settled(&fixture, &id, "finished");
        assert_eq!(failed["status"]["terminalResult"]["status"], "failed");
        let first = observed(&fixture);
        let folder = Path::new(first["paths"]["folder"].as_str().unwrap());
        let key_root = folder.parent().unwrap();
        fs::write(source.join("data.csv"), "changed original").unwrap();
        fs::write(folder.join("data.csv"), "changed copy").unwrap();
        fs::remove_file(fixture.root.path("ready")).unwrap();
        fs::set_permissions(folder, fs::Permissions::from_mode(mode)).unwrap();
        fs::set_permissions(key_root, fs::Permissions::from_mode(mode)).unwrap();
        let output = fixture
            .command(&["resume", &id, "--restart"])
            .output()
            .unwrap();
        if !output.status.success() {
            // Restore only our own fixture on failure so baseline evidence remains disposable.
            fs::set_permissions(key_root, fs::Permissions::from_mode(0o700)).unwrap();
            fs::set_permissions(folder, fs::Permissions::from_mode(0o700)).unwrap();
            assert!(
                !fixture.root.path("ready").exists(),
                "no dispatch before preparation"
            );
            failures.push((mode, String::from_utf8_lossy(&output.stderr).into_owned()));
            continue;
        }
        let part = receipt(&output);
        assert_ne!(part, id);
        let status = settled(&fixture, &part, "finished");
        assert_eq!(
            status["status"]["terminalResult"]["status"], "succeeded",
            "mode {mode:o}: {status}"
        );
        let result = observed(&fixture);
        for field in ["left", "right", "folder"] {
            assert_eq!(result[field], "accepted input");
            assert!(!Path::new(result["paths"][field].as_str().unwrap()).exists());
        }
        assert!(fixture.root.path("ready").exists(), "successor dispatched");
        assert_eq!(
            fs::read(source.join("data.csv")).unwrap(),
            b"changed original"
        );
        assert_eq!(
            fs::read(fixture.workspace.join("live-output")).unwrap(),
            b"worker output"
        );
    }
    assert!(
        failures.is_empty(),
        "restart preparation failures: {failures:?}"
    );
}

#[test]
fn answered_request_refusals_reclaim_only_declared_uploads() {
    let fixture = Fixture::new();
    configure(&fixture, true);
    let mut graph: Value =
        serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
    for name in ["kind", "runId", "operation"] {
        graph["root"]["children"][0]["output"]["fields"][name] =
            json!({"required":false,"type":{"kind":"string"}});
    }
    fixture.write("graph.json", &graph);
    let source = fixture.root.path("data.csv");
    fs::write(&source, "accepted bytes").unwrap();
    fixture.write(
        "input.json",
        &json!({"payload":{"text":"Supply a file", "left":source,"right":source}}),
    );
    let id = receipt(
        &fixture
            .run_command("finish", true)
            .arg("--workspace")
            .output()
            .unwrap(),
    );
    settled(&fixture, &id, "suspended");
    fixture.write(
        "answer.json",
        &json!({"signals":{},"output":{"reply":source}}),
    );
    let successor = receipt(
        &fixture
            .command(&["answer", &id, "--input"])
            .arg(fixture.root.path("answer.json"))
            .output()
            .unwrap(),
    );
    await_file(&fixture.root.path("ready"));
    // Keep the accepted successor's files live while refusing new answers to its predecessor.
    let mut retained_objects = 0;
    let mut failures = Vec::new();
    for explicit_part in [false, true] {
        let upload = fixture.json(&["file", "stage", source.to_str().unwrap()]);
        let mut output = fixture.json(&["file", "stage", source.to_str().unwrap()]);
        output["reply"] = upload;
        fixture.write("answer.json", &json!({"signals":{},"output":output}));
        let before = fs::read_dir(fixture.root.path("state/files/objects"))
            .unwrap()
            .count();
        let mut command = fixture.command(&["answer", &id, "--input"]);
        command.arg(fixture.root.path("answer.json"));
        if explicit_part {
            command.args(["--part", &id]);
        }
        let refused = command.output().unwrap();
        assert!(!refused.status.success());
        let after = fs::read_dir(fixture.root.path("state/files/objects"))
            .unwrap()
            .count();
        if after != before - 1 {
            failures.push((
                explicit_part,
                before,
                after,
                String::from_utf8_lossy(&refused.stderr).into_owned(),
            ));
        }
        retained_objects += 1;
        assert_eq!(fixture.json(&["status", &id])["currentPartId"], successor);
    }
    fs::write(fixture.root.path("release"), "release").unwrap();
    assert_eq!(
        settled(&fixture, &successor, "finished")["status"]["terminalResult"]["status"],
        "succeeded"
    );
    assert_eq!(observed(&fixture)["reply"], "accepted bytes");
    assert!(
        failures.is_empty(),
        "refused upload cleanup failures: {failures:?}"
    );
    assert_eq!(
        fs::read_dir(fixture.root.path("state/files/objects"))
            .unwrap()
            .count(),
        retained_objects
    );
}

#[test]
#[cfg(unix)]
fn startup_keeps_host_input_location_after_variables_and_connections() {
    for override_source in ["none", "variable", "connection"] {
        let fixture = Fixture::new();
        let source = fixture.workspace.join("data.csv");
        fs::write(&source, "accepted bytes").unwrap();
        let schema = json!({"kind":"record","fields":{
            "data":{"required":true,"type":{"kind":"filepath"}}
        }});
        fixture.write(
            "graph.json",
            &json!({"profile":"openengine.graph.full/v1",
            "initialInput":schema,"policy":{"policy":"policy.native-v2@1","default":"deny"},
            "root":{"kind":"seq","name":"root","state":schema,"promotedStatePaths":[],
            "children":[{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}]}}),
        );
        fixture.write("input.json", &json!({"data":source}));
        fixture.write(
            "runtime.json",
            &json!({"harness":"codex","provider":"openai","size":"host",
            "workspaceCheckpoints":false,"activeMemory":{"repository":false,"graph":false},
            "globalAgents":{"postmortem":{"enabled":false}},"nodes":{}}),
        );
        // The marker is in the user's folder and survives accepted-input cleanup.
        let mut environment =
            json!({"startup":"printf '%s' \"$ZEROSHOT_INPUTS\" > input-location"});
        let foreign = fixture.root.path("foreign");
        fs::create_dir(&foreign).unwrap();
        if override_source == "variable" {
            environment["variables"] = json!({"ZEROSHOT_INPUTS":foreign});
        } else if override_source == "connection" {
            environment["connections"] = json!({"storage":["ZEROSHOT_INPUTS"]});
            use std::io::Write;
            let mut child = fixture
                .command(&["connection", "set", "storage", "--json-stdin"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(&serde_json::to_vec(&json!({"ZEROSHOT_INPUTS":foreign})).unwrap())
                .unwrap();
            success(&child.wait_with_output().unwrap());
        }
        fixture.write("environment.json", &environment);
        let id = receipt(
            &fixture
                .run_command("finish", true)
                .args(["--workspace", "--environment"])
                .arg(fixture.root.path("environment.json"))
                .env("ZEROSHOT_INPUTS", &foreign)
                .output()
                .unwrap(),
        );
        let status = settled(&fixture, &id, "finished");
        assert_eq!(
            status["status"]["terminalResult"]["status"], "succeeded",
            "{status}"
        );
        let recovery: Value = serde_json::from_slice(
            &fs::read(
                fixture
                    .root
                    .path(&format!("state/runs/{id}/workspace-recovery.json")),
            )
            .unwrap(),
        )
        .unwrap();
        let accepted = Path::new(
            recovery["submission"]["initialInput"]["data"]
                .as_str()
                .unwrap(),
        );
        let location = fs::read_to_string(fixture.workspace.join("input-location")).unwrap();
        assert_eq!(
            Path::new(&location),
            accepted.parent().unwrap().parent().unwrap(),
            "{override_source}"
        );
        assert!(!accepted.exists());
        assert_eq!(fs::read(&source).unwrap(), b"accepted bytes");
        assert_eq!(
            fs::read_dir(fixture.root.path("state/files/objects"))
                .unwrap()
                .count(),
            0
        );
        assert!(foreign.is_dir());
    }
}

#[test]
fn scalar_submissions_and_answers_preserve_upload_shaped_string_records() {
    for mode in ["submission", "answer", "refused", "late"] {
        let fixture = Fixture::new();
        let source = fixture.root.path("unrelated-upload");
        fs::write(&source, "independent private upload").unwrap();
        let upload = fixture.json(&["file", "stage", source.to_str().unwrap()]);
        let scalar = json!({"kind":"record","fields":{
            "kind":{"required":true,"type":{"kind":"string"}},
            "runId":{"required":true,"type":{"kind":"string"}},
            "operation":{"required":true,"type":{"kind":"string"}}
        }});
        let request = mode != "submission";
        let initial = if request {
            json!({"kind":"record","fields":{"text":{"required":true,"type":{"kind":"string"}}}})
        } else {
            scalar.clone()
        };
        let mut children = vec![];
        if request {
            children.push(json!({"kind":"verifier","name":"approval","worker":"builtin.request.ask@1",
                "input":initial,"output":scalar,"signals":{},
                "diagnostic":{"kind":"record","fields":{"message":{"required":true,"type":{"kind":"string"}}}},
                "attempts":1,"timeoutMs":if mode == "late" {1} else {60000},
                "inputBindings":[{"target":["text"],"value":{"source":"state","path":["text"]}}],
                "writeBindings":[]}));
        }
        children
            .push(json!({"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}));
        fixture.write("graph.json", &json!({"profile":"openengine.graph.full/v1",
            "initialInput":initial,"policy":{"policy":"policy.native-v2@1","default":"deny"},
            "root":{"kind":"seq","name":"root","state":initial,"children":children,"promotedStatePaths":[]}}));
        fixture.write(
            "input.json",
            &if request {
                json!({"text":"Supply ordinary strings"})
            } else {
                upload.clone()
            },
        );
        fixture.write(
            "runtime.json",
            &json!({"harness":"codex","provider":"openai","size":"host",
            "workspaceCheckpoints":request,"activeMemory":{"repository":false,"graph":false},
            "globalAgents":{"postmortem":{"enabled":false}},
            "nodes":if request {json!({"approval":{"kind":"request"}})} else {json!({})}}),
        );
        let id = receipt(
            &fixture
                .run_command("finish", true)
                .arg("--workspace")
                .output()
                .unwrap(),
        );
        if request {
            settled(&fixture, &id, "suspended");
            fixture.write("answer.json", &json!({"signals":if mode == "refused" {json!({"unknown":"label"})} else {json!({})},"output":upload}));
            let answer = fixture
                .command(&["answer", &id, "--input"])
                .arg(fixture.root.path("answer.json"))
                .output()
                .unwrap();
            assert_eq!(
                answer.status.success(),
                mode == "answer",
                "{mode}: {}",
                String::from_utf8_lossy(&answer.stderr)
            );
        }
        if mode == "refused" {
            settled(&fixture, &id, "suspended");
            fixture.json(&["force-stop", &id]);
        }
        let status = settled(&fixture, &id, "finished");
        if matches!(mode, "submission" | "answer") {
            assert_eq!(
                status["status"]["terminalResult"]["status"], "succeeded",
                "{status}"
            );
        }
        assert_eq!(
            fs::read_dir(fixture.root.path("state/files/objects"))
                .unwrap()
                .count(),
            1,
            "{mode} released an upload referenced only by ordinary string fields"
        );
        assert_eq!(fs::read(source).unwrap(), b"independent private upload");
    }
}

#[test]
fn successful_run_releases_readonly_input_directories_and_preserves_originals() {
    for readonly in [false, true] {
        let fixture = Fixture::new();
        let input =
            json!({"kind":"record","fields":{"data":{"required":true,"type":{"kind":"filepath"}}}});
        let mut graph: Value =
            serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
        let mut worker = graph["root"]["children"][0].clone();
        worker.as_object_mut().unwrap().remove("instructions");
        worker["input"] = input.clone();
        worker["inputBindings"] =
            json!([{"target":["data"],"value":{"source":"state","path":["data"]}}]);
        graph["initialInput"] = input.clone();
        graph["root"]["state"] = input;
        graph["root"]["children"] =
            json!([worker,{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}]);
        fixture.write("graph.json", &graph);
        let original = fixture.workspace.join("dataset");
        fs::create_dir_all(original.join("nested")).unwrap();
        fs::write(original.join("nested/data.csv"), "accepted bytes").unwrap();
        let original_permissions = fs::metadata(&original).unwrap().permissions();
        fixture.write("input.json", &json!({"data":"dataset"}));
        let node = std::process::Command::new("node")
            .args(["-p", "process.execPath"])
            .output()
            .unwrap();
        success(&node);
        let node = String::from_utf8(node.stdout).unwrap();
        let observed = serde_json::to_string(&fixture.root.path("observed.json")).unwrap();
        let script = format!(
            r#"
            const fs=require('node:fs'),path=require('node:path');
            const input=JSON.parse(fs.readFileSync(0,'utf8')).data;
            if(fs.readFileSync(path.join(input,'nested/data.csv'),'utf8')!=='accepted bytes') process.exit(42);
            if({readonly}) {{
                fs.chmodSync(path.join(input,'nested/data.csv'),0o444);
                fs.chmodSync(path.join(input,'nested'),0o555);
                fs.chmodSync(input,0o555);
            }}
            fs.writeFileSync({observed},JSON.stringify({{input}}));
            console.log('null');
        "#
        );
        fixture.write(
            "runtime.json",
            &json!({"harness":"codex","provider":"openai","size":"host",
            "workspaceCheckpoints":false,"activeMemory":{"repository":false,"graph":false},
            "globalAgents":{"postmortem":{"enabled":false}},
            "nodes":{"worker":{"kind":"script","command":[node.trim(),"-e",script]}}}),
        );
        let id = receipt(
            &fixture
                .run_command("finish", true)
                .arg("--workspace")
                .output()
                .unwrap(),
        );
        let status = settled(&fixture, &id, "finished");
        assert_eq!(
            status["status"]["terminalResult"]["status"], "succeeded",
            "{status}"
        );
        let value: Value =
            serde_json::from_slice(&fs::read(fixture.root.path("observed.json")).unwrap()).unwrap();
        assert!(!Path::new(value["input"].as_str().unwrap()).exists());
        assert_eq!(
            fs::read_dir(fixture.root.path("state/files/objects"))
                .unwrap()
                .count(),
            0
        );
        assert_eq!(
            fs::read(original.join("nested/data.csv")).unwrap(),
            b"accepted bytes"
        );
        assert_eq!(
            fs::metadata(original).unwrap().permissions(),
            original_permissions
        );
    }
}

#[test]
fn ordinary_state_path_allows_input_filepath_outputs_and_refuses_foreign_or_redirected_roots() {
    for mode in ["accepted", "foreign", "redirected"] {
        let fixture = Fixture::new();
        let state = fixture.root.path("state");
        // Windows canonicalization adds an extended-length prefix. Exercise the ordinary
        // drive spelling supplied by LOCALAPPDATA/ZEROSHOT_STATE_DIR.
        #[cfg(windows)]
        let state = {
            let value = state.to_str().unwrap();
            let state = std::path::PathBuf::from(value.strip_prefix(r"\\?\").unwrap_or(value));
            assert!(
                matches!(state.components().next(), Some(std::path::Component::Prefix(prefix))
                if matches!(prefix.kind(), std::path::Prefix::Disk(_)))
            );
            state
        };
        let input =
            json!({"kind":"record","fields":{"data":{"required":true,"type":{"kind":"filepath"}}}});
        let mut graph: Value =
            serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
        let mut worker = graph["root"]["children"][0].clone();
        worker.as_object_mut().unwrap().remove("instructions");
        worker["input"] = input.clone();
        worker["inputBindings"] =
            json!([{"target":["data"],"value":{"source":"state","path":["data"]}}]);
        worker["output"] = json!({"kind":"filepath"});
        worker["writeBindings"] = json!([]);
        graph["initialInput"] = input.clone();
        graph["root"]["state"] = input;
        graph["root"]["children"] =
            json!([worker,{"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}]);
        fixture.write("graph.json", &graph);
        fs::write(fixture.workspace.join("data.csv"), "accepted bytes").unwrap();
        fixture.write("input.json", &json!({"data":"data.csv"}));
        let outside = fixture.root.path("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("data.csv"), "foreign bytes").unwrap();
        let node = std::process::Command::new("node")
            .args(["-p", "process.execPath"])
            .output()
            .unwrap();
        success(&node);
        let node = String::from_utf8(node.stdout).unwrap();
        let outside = serde_json::to_string(&outside).unwrap();
        let script = format!(
            r#"
            const fs=require('node:fs'), path=require('node:path');
            const input=JSON.parse(fs.readFileSync(0,'utf8')).data;
            if(fs.readFileSync(input,'utf8')!=='accepted bytes') process.exit(42);
            let output=input;
            if('{mode}'==='foreign') output=path.join({outside},'data.csv');
            if('{mode}'==='redirected') {{
                const root=path.dirname(input);
                fs.copyFileSync(input,path.join({outside},path.basename(input)));
                fs.renameSync(root,root+'-moved');
                fs.symlinkSync({outside},root,process.platform==='win32'?'junction':'dir');
            }}
            console.log(JSON.stringify(output));
        "#
        );
        fixture.write(
            "runtime.json",
            &json!({"harness":"codex","provider":"openai","size":"host",
            "workspaceCheckpoints":false,"activeMemory":{"repository":false,"graph":false},
            "globalAgents":{"postmortem":{"enabled":false}},
            "nodes":{"worker":{"kind":"script","command":[node.trim(),"-e",script]}}}),
        );
        let output = fixture
            .run_command("finish", true)
            .arg("--workspace")
            .env("ZEROSHOT_STATE_DIR", &state)
            .output()
            .unwrap();
        let id = receipt(&output);
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let status = fixture
                .command(&["status", &id])
                .env("ZEROSHOT_STATE_DIR", &state)
                .output()
                .unwrap();
            success(&status);
            let status: Value = serde_json::from_slice(&status.stdout).unwrap();
            if status["status"]["phase"] == "finished" {
                assert_eq!(
                    status["status"]["terminalResult"]["status"],
                    if mode == "accepted" {
                        "succeeded"
                    } else {
                        "failed"
                    },
                    "{mode}: {status}"
                );
                break;
            }
            assert!(Instant::now() < deadline, "{mode}: {status}");
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(
            fs::read(fixture.workspace.join("data.csv")).unwrap(),
            b"accepted bytes"
        );
    }
}

#[test]
fn private_uploads_freeze_equal_basenames_and_directories_for_keyed_cli_submission() {
    for managed in [false, true] {
        let fixture = Fixture::new().outside_workspace();
        configure(&fixture, false);
        let mut input = json!({"text":"Read files"});
        for (field, bytes) in [("left", "accepted left"), ("right", "accepted right")] {
            let dir = fixture.root.path(field);
            fs::create_dir(&dir).unwrap();
            fs::write(dir.join("data.csv"), bytes).unwrap();
            input[field] = fixture.json(&["file", "stage", dir.join("data.csv").to_str().unwrap()]);
        }
        input["folder"] =
            fixture.json(&["file", "stage", fixture.root.path("left").to_str().unwrap()]);
        fixture.write("input.json", &json!({"payload":input}));
        let mut command = fixture.run_command("finish", true);
        command.args(["--workspace", "--submission-key", "frozen-files"]);
        if managed {
            command.arg("--copy-workspace");
        }
        let id = receipt(&command.output().unwrap());
        await_file(&fixture.root.path("ready"));
        fs::write(fixture.root.path("left/data.csv"), "changed left").unwrap();
        fs::remove_dir_all(fixture.root.path("right")).unwrap();
        assert_eq!(receipt(&command.output().unwrap()), id);
        let mut missing = input.clone();
        missing.as_object_mut().unwrap().remove("right");
        fixture.write("input.json", &json!({"payload":missing}));
        let refusal = command.output().unwrap();
        assert!(!refusal.status.success());
        assert!(String::from_utf8_lossy(&refusal.stderr).contains("right"));
        fs::write(fixture.root.path("release"), "release").unwrap();
        let status = settled(&fixture, &id, "finished");
        assert_eq!(
            status["status"]["terminalResult"]["status"], "succeeded",
            "{status}"
        );
        let result = observed(&fixture);
        assert_eq!(result["left"], "accepted left");
        assert_eq!(result["right"], "accepted right");
        assert_eq!(result["folder"], "accepted left");
        assert_ne!(result["paths"]["left"], result["paths"]["right"]);
        for field in ["left", "right", "folder"] {
            assert!(!Path::new(result["paths"][field].as_str().unwrap()).exists());
        }
        assert_eq!(
            fs::read(fixture.root.path("left/data.csv")).unwrap(),
            b"changed left"
        );
    }
}

#[test]
fn in_place_answer_files_survive_failed_successor_and_checkpoint_discard() {
    let fixture = Fixture::new();
    configure(&fixture, true);
    let original = fixture.workspace.join("data.csv");
    fs::write(&original, "accepted input").unwrap();
    fixture.write(
        "input.json",
        &json!({"payload":{"text":"Supply a file", "left":"data.csv","right":"data.csv"}}),
    );
    let id = receipt(
        &fixture
            .run_command("finish", true)
            .arg("--workspace")
            .output()
            .unwrap(),
    );
    settled(&fixture, &id, "suspended");
    let recovery: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .root
                .path(&format!("state/runs/{id}/workspace-recovery.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let current = recovery["submission"]["initialInput"]["payload"]["left"]
        .as_str()
        .unwrap();
    fs::write(current, "continued input edit").unwrap();
    fs::write(&original, "accepted answer").unwrap();
    fixture.write(
        "answer.json",
        &json!({"signals":{},"output":{"reply":"data.csv"}}),
    );
    fs::write(fixture.root.path("fail-once"), "fail").unwrap();
    let part = receipt(
        &fixture
            .command(&["answer", &id, "--input"])
            .arg(fixture.root.path("answer.json"))
            .output()
            .unwrap(),
    );
    await_file(&fixture.root.path("ready"));
    fs::write(&original, "user's latest edit").unwrap();
    fs::write(fixture.root.path("release"), "release").unwrap();
    let failed = settled(&fixture, &part, "finished");
    assert_eq!(failed["status"]["terminalResult"]["status"], "failed");
    let first = observed(&fixture);
    assert_eq!(first["left"], "continued input edit");
    assert_eq!(first["reply"], "accepted answer");
    success(
        &fixture
            .command(&["discard-workspace", &part])
            .output()
            .unwrap(),
    );
    assert!(Path::new(first["paths"]["reply"].as_str().unwrap()).exists());
    fs::write(
        first["paths"]["left"].as_str().unwrap(),
        "discarded input edit",
    )
    .unwrap();
    fs::write(
        first["paths"]["reply"].as_str().unwrap(),
        "discarded answer edit",
    )
    .unwrap();
    let restarted = receipt(
        &fixture
            .command(&["resume", &part, "--restart"])
            .output()
            .unwrap(),
    );
    let status = settled(&fixture, &restarted, "finished");
    assert_eq!(
        status["status"]["terminalResult"]["status"], "succeeded",
        "{status}"
    );
    let result = observed(&fixture);
    assert_eq!(result["left"], "accepted input");
    assert_eq!(result["reply"], "accepted answer");
    assert!(!Path::new(result["paths"]["left"].as_str().unwrap()).exists());
    assert!(!Path::new(result["paths"]["reply"].as_str().unwrap()).exists());
    assert_eq!(fs::read(original).unwrap(), b"user's latest edit");
    assert_eq!(
        fs::read(fixture.workspace.join("live-output")).unwrap(),
        b"worker output"
    );
}

#[test]
fn rejected_file_answers_release_uploads_without_prior_file_inputs() {
    for late in [false, true] {
        let fixture = Fixture::new();
        configure(&fixture, true);
        let text =
            json!({"kind":"record","fields":{"text":{"required":true,"type":{"kind":"string"}}}});
        let mut graph: Value =
            serde_json::from_slice(&fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
        let mut approval = graph["root"]["children"][0].clone();
        approval["inputBindings"][0]["value"]["path"] = json!(["text"]);
        approval["output"]["fields"]["optional"] =
            json!({"required":false,"type":{"kind":"filepath"}});
        approval["writeBindings"] = json!([]);
        if late {
            approval["timeoutMs"] = json!(1);
        }
        graph["initialInput"] = text.clone();
        graph["root"]["state"] = text;
        graph["root"]["children"] = json!([approval,
        {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}]);
        fixture.write("graph.json", &graph);
        let mut runtime: Value =
            serde_json::from_slice(&fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
        runtime["nodes"].as_object_mut().unwrap().remove("worker");
        fixture.write("runtime.json", &runtime);
        fixture.write("input.json", &json!({"text":"Supply a file"}));
        let id = receipt(
            &fixture
                .run_command("finish", true)
                .arg("--workspace")
                .output()
                .unwrap(),
        );
        settled(&fixture, &id, "suspended");
        assert!(
            !fixture
                .root
                .path("state/workspace-inputs/file-inputs")
                .exists()
        );
        let source = fixture.root.path("answer.csv");
        fs::write(&source, "private rejected bytes").unwrap();
        let upload = fixture.json(&["file", "stage", source.to_str().unwrap()]);
        let objects = fixture.root.path("state/files/objects");
        assert_eq!(fs::read_dir(&objects).unwrap().count(), 1);
        let output = if late {
            json!({"reply":upload})
        } else {
            json!({"optional":upload})
        };
        fixture.write("answer.json", &json!({"signals":{},"output":output}));
        let refusal = fixture
            .command(&["answer", &id, "--input"])
            .arg(fixture.root.path("answer.json"))
            .output()
            .unwrap();
        assert!(!refusal.status.success());
        if !late {
            assert!(String::from_utf8_lossy(&refusal.stderr).contains("reply"));
            settled(&fixture, &id, "suspended");
        }
        assert_eq!(
            fs::read_dir(objects).unwrap().count(),
            0,
            "{}",
            String::from_utf8_lossy(&refusal.stderr)
        );
        let current = fixture.json(&["status", &id]);
        if late {
            assert_ne!(current["currentPartId"], id, "expiry owns one successor");
        } else {
            assert_eq!(current["currentPartId"], id);
            success(&fixture.command(&["force-stop", &id]).output().unwrap());
        }
    }
}
