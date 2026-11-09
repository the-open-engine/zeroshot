//! Unavailable local `codex app-server` inspection must stay visible while native permissions apply.

use std::time::Duration;

use super::*;
use crate::native_v2_runner::{DurableNodeEvent, LiveOutput, LiveOutputStream};

const CREDENTIAL: &str = "codex-inspection-credential-4d1c";
const PRIVATE_CONFIGURATION: &str = "private-configuration-marker";
const BYPASS: &str = "--dangerously-bypass-approvals-and-sandbox";
const PAST_INSPECTION_BUDGET: Duration = Duration::from_secs(61);

const SCRIPT_PREFIX: &str = r#"#!/bin/sh
set -eu
if [ "${1-}" = app-server ]; then
  printf '%s\n' "${CODEX_API_KEY-unset}" > probe-credential
@PROBE@
  exit 0
fi
cat > /dev/null
printf '%s\n' "$@" > turn-arguments
"#;

const RESPONDER: &str = r#"  while IFS= read -r request; do
    case "$request" in
      *'"method":"initialize"'*) printf '%s\n' '{"id":1,"result":{}}' ;;
      *'"method":"config/read"'*) @CONFIG_READ@ ;;
      *'"method":"configRequirements/read"'*) printf '%s\n' '{"id":3,"result":{"requirements":null}}' ;;
    esac
  done"#;

const HANG: &str = r#"  sleep 600 & child=$!
  printf '%s\n' "$child" > probe-child-pid
  wait"#;

fn script(probe: &str) -> String {
    script_with_success(&SCRIPT_PREFIX.replace("@PROBE@", probe))
}

fn responder(config_read: &str) -> String {
    RESPONDER.replace("@CONFIG_READ@", config_read)
}

fn config_read(config: &str) -> String {
    format!(
        "printf '%s\\n' '{{\"id\":2,\"result\":{{\"config\":{config},\"origins\":{{}},\"layers\":[]}}}}'"
    )
}

/// Native error text is adversarial: it echoes the declared credential, carries terminal and NUL
/// escapes, and is far larger than any log record.
const JSON_RPC_ERROR: &str = r#"filler=$(head -c 65536 /dev/zero | tr '\0' x); printf '{"id":2,"error":{"code":-32603,"message":"failed to load config for %s \\u001b[31m\\u0000 %s"}}\n' "${CODEX_API_KEY-unset}" "$filler""#;

#[derive(Clone, Copy, Debug)]
enum Probe {
    Unset,
    Configured,
    SpawnFailure,
    EarlyExit,
    ExchangeFailure,
    JsonRpcError,
    Malformed,
    Incomplete,
    ByteLimit,
    Timeout,
}

impl Probe {
    fn script(self) -> String {
        match self {
            Self::Unset => script(&responder(&config_read(
                r#"{"sandbox_mode":null,"approval_policy":null}"#,
            ))),
            Self::Configured => script(&responder(&config_read(
                r#"{"sandbox_mode":"workspace-write","approval_policy":null}"#,
            ))),
            Self::SpawnFailure => String::new(),
            Self::EarlyExit => script(
                r#"  printf '%s\n' "failed to initialize sqlite state runtime for ${CODEX_API_KEY-unset}" >&2
  exit 1"#,
            ),
            Self::ExchangeFailure => script(
                r#"  IFS= read -r request
  printf '%s\n' '{"id":1,"result":{}}'"#,
            ),
            Self::JsonRpcError => script(&responder(JSON_RPC_ERROR)),
            Self::Malformed => script(&format!(
                "  printf '%s\\n' '{PRIVATE_CONFIGURATION} not-json'\n  cat > /dev/null"
            )),
            Self::Incomplete => script(&responder(&config_read(&format!(
                r#"{{"approval_policy":null,"private_note":"{PRIVATE_CONFIGURATION}"}}"#
            )))),
            Self::ByteLimit => script("  head -c 4194305 /dev/zero\n  cat > /dev/null"),
            Self::Timeout => script(HANG),
        }
    }
}

struct Observed {
    directory: TestDirectory,
    records: Vec<LiveOutput>,
    completion: Result<WorkerOutcome, NodeRunnerError>,
}

impl Observed {
    fn workspace(&self) -> PathBuf {
        self.directory.child("workspace")
    }

    fn turn_arguments(&self) -> String {
        fs::read_to_string(self.workspace().join("turn-arguments")).assert_value()
    }

    /// Visible, node-associated fallback records: neither debug-only system lines nor model output.
    fn fallback_diagnostics(&self) -> Vec<&LiveOutput> {
        self.records
            .iter()
            .filter(|record| {
                !matches!(
                    record.stream,
                    LiveOutputStream::System | LiveOutputStream::Output
                )
            })
            .filter(|record| {
                let text = record.text.to_lowercase();
                text.contains("permission") && text.contains("bypass")
            })
            .collect()
    }
}

async fn observe(probe: Probe) -> Observed {
    let directory = TestDirectory::new("codex-permission-inspection");
    let runtime_home = directory.child("runtime-home");
    fs::create_dir_all(&runtime_home).assert_value();
    let executable = match probe {
        Probe::SpawnFailure => directory.child("missing-codex"),
        _ => directory.write_executable("codex-script", &probe.script()),
    };
    let scripted =
        adapter_with_configuration(&directory, CodexProvider::OpenAi, executable, runtime_home);
    // Use real native inspection, without the transcript fixture's injected permission policy.
    let adapter = Arc::new(NativeV2CodexAdapter::new_local(scripted.config.clone()));
    let admitted = admitted(
        binding(SessionScope::Execution, &["OPENAI_API_KEY"]),
        CodexProvider::OpenAi,
    )
    .await;
    let runtime = runner(&admitted, adapter);
    let mut handle = start(
        &runtime,
        &admitted,
        1,
        &[("OPENAI_API_KEY", CREDENTIAL.to_owned())],
    )
    .await;
    let mut output = handle.take_initial_output().assert_value();
    let collector = tokio::spawn(async move {
        let mut records = Vec::new();
        while let Ok(event) = output.recv().await {
            if let DurableNodeEvent::Output { output, .. } = event {
                records.push(output);
            }
        }
        records
    });
    if matches!(probe, Probe::Timeout) {
        wait_for_file(&directory.child("workspace").join("probe-child-pid")).await;
        // Arm the real probe deadline, then resume before OS cleanup needs polling.
        tokio::time::pause();
        tokio::time::advance(PAST_INSPECTION_BUDGET).await;
        tokio::time::resume();
    }
    let completion = tokio::time::timeout(Duration::from_secs(60), handle.completion())
        .await
        .assert_value_with("Codex node did not settle");
    let records = collector.await.assert_value();
    Observed {
        directory,
        records,
        completion: completion.map(|completion| completion.outcome),
    }
}

async fn wait_for_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while fs::read_to_string(path)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .is_none()
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .assert_value_with("probe did not start");
}

fn assert_completed_natively(case: Probe, observed: &Observed) {
    assert!(
        matches!(
            observed.completion.as_ref(),
            Ok(WorkerOutcome::Verified { output, .. }) if *output == json!({"answer": 42})
        ),
        "{case:?}: a native fallback must keep the successful result: {:?}",
        observed.completion
    );
}

fn single_fallback_diagnostic(case: Probe, observed: &Observed) -> String {
    let diagnostics = observed.fallback_diagnostics();
    assert_eq!(
        diagnostics.len(),
        1,
        "{case:?}: expected one visible permission-inspection fallback diagnostic, got {:?}",
        observed.records
    );
    let diagnostic = diagnostics[0];
    assert_safe_diagnostic(case, diagnostic);
    diagnostic.text.clone()
}

fn assert_safe_diagnostic(case: Probe, diagnostic: &LiveOutput) {
    assert_eq!(
        diagnostic.diagnostic, None,
        "{case:?}: fallback must not become a failure diagnostic"
    );
    let text = &diagnostic.text;
    assert!(
        text.to_lowercase().contains("codex"),
        "{case:?}: diagnostic must name the harness: {text}"
    );
    assert!(
        !text.contains(CREDENTIAL),
        "{case:?}: diagnostic exposed a credential: {text}"
    );
    assert!(
        !text.contains(PRIVATE_CONFIGURATION),
        "{case:?}: diagnostic exposed private configuration: {text}"
    );
}

#[tokio::test]
async fn unavailable_codex_permission_inspection_reports_each_cause_and_keeps_native_permissions() {
    let mut causes = BTreeMap::new();
    for case in [
        Probe::EarlyExit,
        Probe::ExchangeFailure,
        Probe::JsonRpcError,
        Probe::Malformed,
        Probe::Incomplete,
        Probe::ByteLimit,
        Probe::Timeout,
    ] {
        let observed = observe(case).await;
        assert_eq!(
            fs::read_to_string(observed.workspace().join("probe-credential")).assert_value(),
            format!("{CREDENTIAL}\n"),
            "{case:?}: the probe must see the declared credential it may echo"
        );
        assert_completed_natively(case, &observed);
        assert!(
            !observed.turn_arguments().contains(BYPASS),
            "{case:?}: unavailable inspection enabled the automatic bypass"
        );
        causes.insert(
            format!("{case:?}"),
            single_fallback_diagnostic(case, &observed),
        );
    }

    // A probe that cannot start cannot be followed by a model turn either; the native launch
    // failure stays authoritative, and the fallback is still reported for that inspection.
    let observed = observe(Probe::SpawnFailure).await;
    assert!(observed.completion.is_err(), "{:?}", observed.completion);
    let diagnostics = observed.fallback_diagnostics();
    assert!(
        !diagnostics.is_empty(),
        "SpawnFailure: expected a visible permission-inspection fallback diagnostic, got {:?}",
        observed.records
    );
    for diagnostic in &diagnostics {
        assert_safe_diagnostic(Probe::SpawnFailure, diagnostic);
    }
    causes.insert("SpawnFailure".to_owned(), diagnostics[0].text.clone());

    // Each reported category names its own cause; exchange failures may share early-exit wording
    // and incomplete answers may share malformed-response wording.
    let distinct = [
        "SpawnFailure",
        "EarlyExit",
        "JsonRpcError",
        "Malformed",
        "ByteLimit",
        "Timeout",
    ];
    for (index, left) in distinct.iter().enumerate() {
        for right in &distinct[index + 1..] {
            assert_ne!(
                causes[*left], causes[*right],
                "{left} and {right} must report distinct causes"
            );
        }
    }
}

#[tokio::test]
async fn answered_codex_permission_inspection_reports_no_fallback() {
    for case in [Probe::Unset, Probe::Configured] {
        let observed = observe(case).await;
        assert_completed_natively(case, &observed);
        assert_eq!(
            observed.turn_arguments().contains(BYPASS),
            matches!(case, Probe::Unset),
            "{case:?}: only verified unset policy receives the automatic bypass"
        );
        assert!(
            observed
                .records
                .iter()
                .filter(|record| record.stream != LiveOutputStream::System)
                .all(|record| !record.text.to_lowercase().contains("permission")),
            "{case:?}: answered inspection reported a false fallback: {:?}",
            observed.records
        );
    }
}

#[tokio::test]
async fn cancelled_codex_permission_inspection_never_reports_a_fallback() {
    let directory = TestDirectory::new("codex-permission-inspection-cancel");
    let adapter = scripted_adapter_with(
        &directory,
        CodexProvider::OpenAi,
        "codex-script",
        &Probe::Timeout.script(),
    );
    let adapter = Arc::new(NativeV2CodexAdapter::new_local(adapter.config.clone()));
    let admitted = admitted(
        binding(SessionScope::Execution, &["OPENAI_API_KEY"]),
        CodexProvider::OpenAi,
    )
    .await;
    let runtime = runner(&admitted, adapter);
    let mut handle = start(
        &runtime,
        &admitted,
        1,
        &[("OPENAI_API_KEY", CREDENTIAL.to_owned())],
    )
    .await;
    let mut output = handle.take_initial_output().assert_value();
    let pid_file = directory.child("workspace").join("probe-child-pid");
    wait_for_file(&pid_file).await;
    handle.cancel();
    assert_eq!(handle.completion().await, Err(NodeRunnerError::Cancelled));
    while let Ok(event) = output.recv().await {
        if let DurableNodeEvent::Output { output, .. } = event {
            assert!(
                !output.text.to_lowercase().contains("bypass"),
                "cancellation was reported as a permission fallback: {}",
                output.text
            );
        }
    }
    assert!(!directory.child("workspace").join("turn-arguments").exists());
}
