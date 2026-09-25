use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use openengine_cluster_testkit::TemporaryDirectory;
use serde_json::{Value, json};

pub(super) struct Fixture {
    pub(super) root: TemporaryDirectory,
    pub(super) workspace: PathBuf,
    path: std::ffi::OsString,
}

impl Fixture {
    pub(super) fn new() -> Self {
        // Keep the state root short enough for the controller's Unix-domain socket path.
        let root = TemporaryDirectory::for_test("pc");
        let workspace = root.path("workspace space-é & (native)");
        let bin = root.path("bin space-é");
        std::fs::create_dir(&workspace).unwrap();
        std::fs::create_dir(&bin).unwrap();
        let node = Command::new("node")
            .args(["-p", "process.execPath"])
            .output()
            .unwrap();
        success(&node);
        let node = String::from_utf8(node.stdout).unwrap();
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/native_v2_cli_portable/harness.cjs"
            ),
            bin.join("harness.cjs"),
        )
        .unwrap();
        install_shim(&bin, node.trim());
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        let fixture = Self {
            root,
            workspace,
            path: std::env::join_paths(paths).unwrap(),
        };
        for args in [
            vec!["init"],
            vec!["config", "user.name", "Fixture"],
            vec!["config", "user.email", "fixture@example.invalid"],
            vec!["config", "commit.gpgsign", "false"],
            vec![
                "remote",
                "add",
                "origin",
                "https://github.com/acme/native-fixture.git",
            ],
        ] {
            success(
                &Command::new("git")
                    .current_dir(&fixture.workspace)
                    .args(args)
                    .output()
                    .unwrap(),
            );
        }
        std::fs::write(fixture.workspace.join("seed"), "seed").unwrap();
        success(
            &Command::new("git")
                .current_dir(&fixture.workspace)
                .args(["add", "."])
                .output()
                .unwrap(),
        );
        success(
            &Command::new("git")
                .current_dir(&fixture.workspace)
                .args(["commit", "-m", "seed"])
                .output()
                .unwrap(),
        );
        fixture.write("graph.json", &local_graph::graph());
        fixture.write("input.json", &Value::Null);
        fixture.write(
            "runtime.json",
            &json!({"harness":"codex","provider":"openai","size":"small",
            "nodes":{"worker":{"kind":"agent","model":"fixture-owned-model",
                "connections":{"openai":["OPENAI_API_KEY","FIXTURE_MODE"]}}}}),
        );
        fixture
    }

    pub(super) fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_zeroshot"));
        command
            .args(args)
            .current_dir(&self.workspace)
            .env_clear()
            .env("PATH", &self.path)
            .env("ZEROSHOT_STATE_DIR", self.root.path("state"))
            .env("ZEROSHOT_CONFIG_DIR", self.root.path("config"))
            .env("OPENAI_API_KEY", "fixture-key")
            .env("UNDECLARED_SECRET", "must-not-inherit");
        for name in [
            "SystemRoot",
            "WINDIR",
            "COMSPEC",
            "TEMP",
            "TMP",
            "PATHEXT",
            "USERPROFILE",
            "LOCALAPPDATA",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
    }

    pub(super) fn run(&self, mode: &str, detach: bool) -> Output {
        if mode == "volume" {
            let mut graph = local_graph::graph();
            let payload = json!({"kind":"record","fields":{"blob":{"type":{"kind":"string"},"required":true}}});
            graph["initialInput"] = payload.clone();
            graph["root"]["state"] = payload.clone();
            graph["root"]["children"][0]["input"] = payload;
            graph["root"]["children"][0]["inputBindings"] =
                json!([{"target":["blob"],"value":{"source":"state","path":["blob"]}}]);
            graph["root"]["children"][1]["state"] = graph["root"]["state"].clone();
            self.write("graph.json", &graph);
            self.write("input.json", &json!({"blob":"x".repeat(256 * 1024)}));
        }
        let mut command = self.command(&["run", "--title", "Native portability fixture"]);
        for (option, file) in [
            ("--graph", "graph.json"),
            ("--input", "input.json"),
            ("--runtime-config", "runtime.json"),
        ] {
            command.arg(option).arg(self.root.path(file));
        }
        if detach {
            command.arg("--detach");
        }
        command.env("FIXTURE_MODE", mode).output().unwrap()
    }

    pub(super) fn json(&self, args: &[&str]) -> Value {
        let output = self.command(args).output().unwrap();
        success(&output);
        serde_json::from_slice(&output.stdout).unwrap()
    }

    pub(super) fn write(&self, file: &str, value: &Value) {
        std::fs::write(self.root.path(file), serde_json::to_vec(value).unwrap()).unwrap();
    }

    pub(super) fn start_blocked(&self) -> (String, u32) {
        let output = self.run("block", true);
        success(&output);
        let receipt: Value = serde_json::from_slice(&output.stdout).unwrap();
        (
            receipt["runId"].as_str().unwrap().to_owned(),
            self.await_child(),
        )
    }

    fn await_child(&self) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(pid) = std::fs::read_to_string(self.workspace.join("child.pid")) {
                if let Ok(pid) = pid.parse() {
                    return pid;
                }
            }
            assert!(Instant::now() < deadline, "provider never started");
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

pub(super) fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn install_shim(bin: &Path, node: &str) {
    #[cfg(windows)]
    {
        // npm installs both a POSIX launcher and a Windows command shim.
        std::fs::write(bin.join("codex"), "#!/bin/sh\nexit 42\n").unwrap();
        std::fs::write(
            bin.join("codex.cmd"),
            format!("@echo off\r\n\"{node}\" \"%~dp0harness.cjs\" %*\r\n"),
        )
        .unwrap();
    }
    #[cfg(unix)]
    {
        let shim = bin.join("codex");
        openengine_cluster_testkit::fixture::write_executable(
            &shim,
            format!(
                "#!/bin/sh\nexec '{}' '{}' \"$@\"\n",
                node.replace('\'', "'\\''"),
                bin.join("harness.cjs").display()
            ),
            0o700,
        )
        .unwrap();
    }
}

#[path = "../support/local_graph.rs"]
mod local_graph;
