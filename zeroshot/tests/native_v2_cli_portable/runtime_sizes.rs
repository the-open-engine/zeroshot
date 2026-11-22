//! Local preflight uses the execution catalog without changing portable authored runtimes.

use std::process::Output;

use serde_json::{Value, json};

use super::{Fixture, run_directories, success};

fn write_runtime(fixture: &Fixture, uniform: bool, size: Option<&str>) {
    let mut runtime = json!({
        "harness":"codex", "provider":"openai", "workspaceCheckpoints":false,
        "activeMemory":{"repository":false,"graph":false},
        "globalAgents":{"postmortem":{"enabled":false}}
    });
    if uniform {
        runtime["model"] = json!("fixture-owned-model");
    } else {
        runtime["nodes"] = json!({"worker":{"kind":"agent","model":"fixture-owned-model"}});
    }
    if let Some(size) = size {
        runtime["size"] = json!(size);
    }
    fixture.write("runtime.json", &runtime);
}

fn validate(fixture: &Fixture, uniform: bool, profile: bool, named: bool) -> Output {
    let mut command = fixture.command(&[
        "run",
        "--title",
        "Size preflight",
        "--validate-only",
        "--input",
    ]);
    command.arg(fixture.root.path("input.json"));
    if profile {
        command.args(["--profile", "local:size-probe"]);
    } else {
        command.arg("--graph").arg(fixture.root.path("graph.json"));
        command.arg(if uniform {
            "--uniform-runtime-config"
        } else {
            "--runtime-config"
        });
        command.arg(fixture.root.path("runtime.json"));
    }
    if named {
        command.args(["--target", "unconfigured"]);
    }
    command
        .env("ZEROSHOT_ERROR_FORMAT", "json")
        .output()
        .unwrap()
}

fn assert_unoffered(output: &Output, size: &str) {
    assert!(
        !output.status.success(),
        "unexpected validation: {output:?}"
    );
    assert!(output.stdout.is_empty(), "{output:?}");
    let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(diagnostic["kind"], "invalid_request");
    assert_eq!(diagnostic["code"], "request.invalid");
    let message = diagnostic["message"].as_str().unwrap();
    assert!(
        message.contains(&format!("does not offer run size {size}")),
        "{message}"
    );
    assert!(message.contains("supported sizes: host"), "{message}");
}

fn assert_no_execution(fixture: &Fixture) {
    assert!(run_directories(fixture).is_empty());
    assert!(!fixture.workspace.join("mutation.txt").exists());
    assert!(!fixture.workspace.join("script-started").exists());
    assert!(!fixture.workspace.join("environment-started").exists());
    assert!(!fixture.root.path("state").exists());
}

#[test]
fn local_preflight_rejects_unoffered_sizes_for_exact_and_uniform_runtimes() {
    let fixture = Fixture::new();
    for uniform in [false, true] {
        for size in ["medium", "small", "custom-big-box"] {
            write_runtime(&fixture, uniform, Some(size));
            assert_unoffered(&validate(&fixture, uniform, false, false), size);
            assert_no_execution(&fixture);
        }
    }
}

#[test]
fn local_preflight_accepts_host_and_default_without_execution() {
    let fixture = Fixture::new();
    for uniform in [false, true] {
        for size in [Some("host"), None] {
            write_runtime(&fixture, uniform, size);
            let output = validate(&fixture, uniform, false, false);
            success(&output);
            assert_eq!(output.stdout, b"{\"valid\":true}\n");
            assert_no_execution(&fixture);
        }
    }
}

#[test]
fn saved_profiles_stay_portable_but_local_validation_checks_host_sizes() {
    let fixture = Fixture::new();
    for size in [Some("medium"), Some("host"), None] {
        write_runtime(&fixture, false, size);
        success(
            &fixture
                .command(&["profile", "set", "size-probe", "--graph"])
                .arg(fixture.root.path("graph.json"))
                .arg("--runtime-config")
                .arg(fixture.root.path("runtime.json"))
                .output()
                .unwrap(),
        );
        let shown = fixture.json(&["profile", "show", "size-probe"]);
        assert_eq!(shown["runtime"]["size"].as_str(), size);
        let output = validate(&fixture, false, true, false);
        if size == Some("medium") {
            assert_unoffered(&output, "medium");
        } else {
            success(&output);
            assert_eq!(output.stdout, b"{\"valid\":true}\n");
        }
        assert_no_execution(&fixture);
        // A locally saved profile may carry an opaque size belonging to another target.
        success(&validate(&fixture, false, true, true));
    }
}

#[test]
fn named_preflight_preserves_target_owned_sizes_without_target_contact() {
    let fixture = Fixture::new();
    for uniform in [false, true] {
        for size in [Some("medium"), Some("custom-big-box"), None] {
            write_runtime(&fixture, uniform, size);
            success(&validate(&fixture, uniform, false, true));
            assert_no_execution(&fixture);
        }
    }
}

#[test]
fn script_preflight_has_no_effects_and_host_execution_keeps_submission_identity() {
    let fixture = Fixture::new();
    let mut graph: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.path("graph.json")).unwrap()).unwrap();
    graph["root"]["children"][0]
        .as_object_mut()
        .unwrap()
        .remove("instructions");
    fixture.write("graph.json", &graph);
    for size in [None, Some("host")] {
        write_runtime(&fixture, false, size);
        let mut runtime: Value =
            serde_json::from_slice(&std::fs::read(fixture.root.path("runtime.json")).unwrap())
                .unwrap();
        runtime["nodes"]["worker"] = json!({"kind":"script","command":[
            "node", "-e", "require('fs').writeFileSync('script-started', 'started'); console.log('null')"
        ]});
        fixture.write("runtime.json", &runtime);
        let mut command = fixture.run_command("finish", true);
        command.arg("--validate-only");
        #[cfg(unix)]
        {
            fixture.write(
                "environment.json",
                &json!({"setup":"echo started > environment-started"}),
            );
            command
                .arg("--environment")
                .arg(fixture.root.path("environment.json"));
        }
        success(&command.output().unwrap());
        assert_no_execution(&fixture);
    }
    // The same key is replayable only with the same original size selection, even when
    // omission and explicit host resolve to identical execution resources.
    let submit = || {
        fixture
            .run_command("finish", true)
            .args(["--submission-key", "size-identity"])
            .output()
            .unwrap()
    };
    let first = submit();
    success(&first);
    let receipt: Value = serde_json::from_slice(&first.stdout).unwrap();
    let id = receipt["runId"].as_str().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let status = fixture.json(&["status", id]);
        if status["status"]["phase"] == "finished" {
            assert_eq!(status["size"], "host");
            assert_eq!(status["status"]["terminalResult"]["status"], "succeeded");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{status}");
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    assert!(fixture.workspace.join("script-started").exists());
    let replay = submit();
    success(&replay);
    assert_eq!(
        serde_json::from_slice::<Value>(&replay.stdout).unwrap(),
        receipt
    );
    let mut runtime: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.path("runtime.json")).unwrap()).unwrap();
    runtime.as_object_mut().unwrap().remove("size");
    fixture.write("runtime.json", &runtime);
    let changed = submit();
    assert!(!changed.status.success());
    assert!(
        String::from_utf8_lossy(&changed.stderr).contains("submission key"),
        "{changed:?}"
    );
    assert_eq!(run_directories(&fixture).len(), 1);
}

#[test]
fn python_local_submission_rejects_unoffered_sizes_during_preflight() {
    use std::path::{Path, PathBuf};

    let fixture = Fixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let python = root.join(if cfg!(windows) {
        ".venv/Scripts/python.exe"
    } else {
        ".venv/bin/python"
    });
    let python = if python.is_file() {
        python
    } else {
        PathBuf::from(if cfg!(windows) { "python" } else { "python3" })
    };
    let environment = fixture.command(&[]);
    let output = std::process::Command::new(python)
        .env_clear()
        .envs(
            environment
                .get_envs()
                .filter_map(|(key, value)| value.map(|value| (key, value))),
        )
        .env("PYTHONPATH", root.join("sdks/python/src"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env(
            "ZEROSHOT_PYTHON_NATIVE_BINARY",
            env!("CARGO_BIN_EXE_zeroshot"),
        )
        .current_dir(&fixture.workspace)
        .args(["-c", include_str!("runtime_sizes.py")])
        .arg(fixture.root.path("graph.json"))
        .output()
        .unwrap();
    success(&output);
    assert_no_execution(&fixture);
}
