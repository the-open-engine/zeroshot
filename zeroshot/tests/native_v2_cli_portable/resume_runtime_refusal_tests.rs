use super::{Fixture, success};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

fn assert_refusal(fixture: &Fixture, id: &str, expected: (&str, &str, &str)) {
    let (kind, code, reason) = expected;
    for option in [
        None,
        Some("--runtime-config"),
        Some("--uniform-runtime-config"),
    ] {
        let mut command = fixture.command(&["resume", id]);
        if let Some(option) = option {
            command
                .arg(option)
                .arg(fixture.root.path(if option == "--runtime-config" {
                    "runtime.json"
                } else {
                    "uniform.json"
                }));
        }
        let output = command
            .env("ZEROSHOT_ERROR_FORMAT", "json")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let diagnostic: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(diagnostic["kind"], kind, "{option:?}: {diagnostic}");
        assert_eq!(diagnostic["code"], code, "{option:?}: {diagnostic}");
        assert!(
            diagnostic["message"].as_str().unwrap().contains(reason),
            "{diagnostic}"
        );
    }

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
        .envs(environment.get_envs().filter_map(|(k, v)| v.map(|v| (k, v))))
        .env("PYTHONPATH", root.join("sdks/python/src"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("ZEROSHOT_PYTHON_NATIVE_BINARY", env!("CARGO_BIN_EXE_zeroshot"))
        .current_dir(fixture.root.path(""))
        .args(["-c", r#"import asyncio, json, sys
from zeroshot import Client, LocalTarget, RuntimePlan, UniformRuntime, InvalidRequestError, RunNotFoundError
async def main():
    expected = RunNotFoundError if sys.argv[3] == "run_not_found" else InvalidRequestError
    with open("runtime.json", encoding="utf-8") as file:
        exact = RuntimePlan(json.load(file))
    uniform = UniformRuntime(harness="codex", provider="openai", model="fixture-owned-model")
    async with Client(target=LocalTarget(state_dir=sys.argv[1])) as client:
        for runtime in (None, exact, uniform):
            try:
                await client.get_run(sys.argv[2]).resume(runtime=runtime)
            except expected as error:
                assert type(error) is expected, type(error)
                assert sys.argv[4] in str(error), str(error)
            else:
                raise AssertionError("resume unexpectedly succeeded")
asyncio.run(main())
"#])
        .arg(fixture.root.path("state"))
        .args([id, kind, reason])
        .output()
        .unwrap();
    success(&output);
}

#[test]
fn resume_runtime_preserves_cli_and_sdk_refusal_types() {
    let fixture = Fixture::new().outside_workspace();
    fixture.write(
        "uniform.json",
        &json!({"harness":"codex","provider":"openai","model":"fixture-owned-model"}),
    );
    let missing = "0199f33f-3b44-7d21-9000-000000000001";
    assert_refusal(
        &fixture,
        missing,
        ("run_not_found", "run.not_found", missing),
    );
    assert!(!fixture.root.path(&format!("state/runs/{missing}")).exists());

    let failed = fixture.run("fail", false);
    assert!(!failed.status.success());
    let receipt: Value =
        serde_json::from_slice(failed.stdout.split(|byte| *byte == b'\n').next().unwrap()).unwrap();
    let id = receipt["runId"].as_str().unwrap();
    assert_eq!(fixture.json(&["status", id])["status"]["phase"], "finished");
    let storage = fixture.root.path(&format!("state/runs/{id}"));
    let record = storage.join("workspace-recovery.json");
    let ledger = fs::read(storage.join("runs.sqlite3")).unwrap();
    fs::write(fixture.workspace.join("mutation.txt"), b"operator edits").unwrap();
    fs::remove_file(&record).unwrap();
    assert_refusal(
        &fixture,
        id,
        ("invalid_request", "request.invalid", "is missing"),
    );
    assert!(!record.exists());
    fs::write(&record, b"not json").unwrap();
    assert_refusal(
        &fixture,
        id,
        ("invalid_request", "request.invalid", "invalid"),
    );
    assert_eq!(fs::read(&record).unwrap(), b"not json");
    assert_eq!(fs::read(storage.join("runs.sqlite3")).unwrap(), ledger);
    assert_eq!(
        fs::read(fixture.workspace.join("mutation.txt")).unwrap(),
        b"operator edits"
    );
    assert_eq!(
        fs::read_dir(fixture.root.path("state/runs"))
            .unwrap()
            .count(),
        1
    );
}
