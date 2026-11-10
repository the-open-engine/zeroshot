//! Unavailable local Claude settings inspection must stay visible while native permissions apply.

use std::fs;

use super::*;
use crate::native_v2_runner::{DurableNodeEvent, LiveOutput, LiveOutputStream};

const CREDENTIAL: &str = "claude-inspection-credential-9b2e";
const PRIVATE_CONFIGURATION: &str = "private-configuration-marker";
const BYPASS: &str = "--dangerously-skip-permissions";
const PAST_INSPECTION_BUDGET: Duration = Duration::from_secs(61);

const SCRIPT: &str = r#"#!/bin/sh
set -eu
if [ "$*" = --version ]; then printf '%s\n' '2.1.318 (Claude Code)'; exit 0; fi
for argument in "$@"; do
  if [ "$argument" = --safe-mode ]; then
    printf '%s\n' "${ANTHROPIC_API_KEY-unset}" > probe-credential
@PROBE@
    exit 0
  fi
done
cat > /dev/null
printf '%s\n' "$@" > turn-arguments
printf '%s\n' '{"type":"system","subtype":"init","session_id":"session-1"}'
printf '%s%s%s\n' \
  '{"type":"result","subtype":"success","is_error":false,"result":"{\"response\":\"done\"}",' \
  '"session_id":"session-1","usage":{"input_tokens":11,"output_tokens":4,"cache_read_input_tokens":6,' \
  '"cache_creation_input_tokens":2},"model_usage_since_last_result":{"model":{"inputTokens":11,"outputTokens":4,"cacheReadInputTokens":6,"cacheCreationInputTokens":2}}}'
"#;

const RESPONDER: &str = r#"    IFS= read -r initialize
    printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"zeroshot-initialize","response":{"current_permission_mode":"default"}}}'
    IFS= read -r settings
@SETTINGS@"#;

const HANG: &str = r#"    sleep 600 & child=$!
    printf '%s\n' "$child" > probe-child-pid
    wait"#;

/// Native error text is adversarial: it echoes the declared credential, carries terminal and NUL
/// escapes, and is far larger than any log record.
const NATIVE_ERROR: &str = r#"    filler=$(head -c 65536 /dev/zero | tr '\0' x)
    printf '{"type":"control_response","response":{"subtype":"error","request_id":"zeroshot-settings","error":"settings could not be loaded for %s \\u001b[31m\\u0000 %s"}}\n' "${ANTHROPIC_API_KEY-unset}" "$filler""#;

fn script(probe: &str) -> String {
    SCRIPT.replace("@PROBE@", probe)
}

fn settings(response: &str) -> String {
    script(&RESPONDER.replace(
        "@SETTINGS@",
        &format!(
            "    printf '%s\\n' '{{\"type\":\"control_response\",\"response\":{{\"subtype\":\"success\",\"request_id\":\"zeroshot-settings\",\"response\":{response}}}}}'"
        ),
    ))
}

#[derive(Clone, Copy, Debug)]
enum Probe {
    Unset,
    Configured,
    EarlyExit,
    ExchangeFailure,
    NativeError,
    Malformed,
    Incomplete,
    ByteLimit,
    Timeout,
}

impl Probe {
    fn script(self) -> String {
        match self {
            Self::Unset => settings(r#"{"effective":{},"sources":[],"errors":[]}"#),
            Self::Configured => settings(
                r#"{"effective":{"permissions":{"defaultMode":"plan"}},"sources":[],"errors":[]}"#,
            ),
            Self::EarlyExit => script(
                r#"    printf '%s\n' "error: unknown option '--safe-mode' ${ANTHROPIC_API_KEY-unset}" >&2
    exit 1"#,
            ),
            Self::ExchangeFailure => script(&RESPONDER.replace("@SETTINGS@", "")),
            Self::NativeError => script(&RESPONDER.replace("@SETTINGS@", NATIVE_ERROR)),
            Self::Malformed => script(&format!(
                "    printf '%s\\n' '{PRIVATE_CONFIGURATION} not-json'\n    cat > /dev/null"
            )),
            Self::Incomplete => settings(&format!(
                r#"{{"effective":{{"privateSetting":"{PRIVATE_CONFIGURATION}"}}}}"#
            )),
            Self::ByteLimit => script("    head -c 4194305 /dev/zero\n    cat > /dev/null"),
            Self::Timeout => script(HANG),
        }
    }
}

struct Inspection {
    workspace: TestDirectory,
    executable: PathBuf,
    runner: NativeNodeRunner,
    binding: NodeRuntimeBinding,
}

struct Observed {
    records: Vec<LiveOutput>,
    completion: Result<WorkerOutcome, NodeRunnerError>,
}

impl Inspection {
    async fn new(probe: Probe) -> Self {
        let workspace = TestDirectory::new("claude-permission-inspection");
        let executable = workspace.write_executable("fake-claude", &probe.script());
        let binding = agent_binding(
            "claude-provider-model",
            None,
            SessionScope::Execution,
            &[ANTHROPIC_KEY],
        );
        let admitted = admitted_claude(ClaudeProvider::Anthropic, binding.clone(), false).await;
        let mut adapter = ClaudeAdapter::new_for_test(ClaudeAdapterConfig {
            provider: ClaudeProvider::Anthropic,
            executable: executable.to_string_lossy().into_owned(),
            prefix_arguments: Vec::new(),
            workspace: workspace.path().to_owned(),
            runtime_home: workspace.path().to_owned(),
            local_user_home: None,
            native_environment: Default::default(),
            base_environment: ClaudeProcessEnvironment::new(BTreeMap::from([
                (
                    "HOME".to_owned(),
                    workspace.path().to_string_lossy().into_owned(),
                ),
                ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ]))
            .assert_value(),
            process_pool: HostedProcessPool::new(10_002, 10_002, 20_000).assert_value(),
        })
        .assert_value();
        // Use real native inspection, without the transcript fixture's injected permission policy.
        adapter.test_permission_policy = None;
        let adapter = Arc::new(adapter);
        let runner = NativeNodeRunner::new(&admitted, adapter.clone(), adapter).assert_value();
        Self {
            workspace,
            executable,
            runner,
            binding,
        }
    }

    async fn observe(&self, execution: u64, hang: bool) -> Observed {
        let mut handle = self
            .runner
            .start(request(
                self.binding.clone(),
                execution,
                &[(ANTHROPIC_KEY, CREDENTIAL)],
            ))
            .await
            .assert_value();
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
        if hang {
            wait_for_file(&self.workspace.child("probe-child-pid")).await;
            // Arm the real probe deadline, then resume before OS cleanup needs polling.
            tokio::time::pause();
            tokio::time::advance(PAST_INSPECTION_BUDGET).await;
            tokio::time::resume();
        }
        let completion = tokio::time::timeout(Duration::from_secs(60), handle.completion())
            .await
            .assert_value_with("Claude node did not settle");
        Observed {
            records: collector.await.assert_value(),
            completion: completion.map(|completion| completion.outcome),
        }
    }

    fn turn_arguments(&self) -> String {
        fs::read_to_string(self.workspace.child("turn-arguments")).assert_value()
    }
}

impl Observed {
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

fn assert_completed_natively(case: &str, observed: &Observed) {
    assert!(
        matches!(
            observed.completion.as_ref(),
            Ok(WorkerOutcome::Verified { output, .. }) if *output == json!("done")
        ),
        "{case}: a native fallback must keep the successful result: {:?}",
        observed.completion
    );
}

fn assert_safe_diagnostic(case: &str, diagnostic: &LiveOutput) {
    assert_eq!(
        diagnostic.diagnostic, None,
        "{case}: fallback must not become a failure diagnostic"
    );
    let text = &diagnostic.text;
    assert!(
        text.to_lowercase().contains("claude"),
        "{case}: diagnostic must name the harness: {text}"
    );
    assert!(
        !text.contains(CREDENTIAL),
        "{case}: diagnostic exposed a credential: {text}"
    );
    assert!(
        !text.contains(PRIVATE_CONFIGURATION),
        "{case}: diagnostic exposed private configuration: {text}"
    );
}

#[tokio::test]
async fn unavailable_claude_permission_inspection_reports_each_cause_and_keeps_native_permissions()
{
    let mut causes = BTreeMap::new();
    for case in [
        Probe::EarlyExit,
        Probe::ExchangeFailure,
        Probe::NativeError,
        Probe::Malformed,
        Probe::Incomplete,
        Probe::ByteLimit,
        Probe::Timeout,
    ] {
        let label = format!("{case:?}");
        let inspection = Inspection::new(case).await;
        let observed = inspection.observe(1, matches!(case, Probe::Timeout)).await;
        assert_eq!(
            inspection.workspace.read("probe-credential"),
            format!("{CREDENTIAL}\n"),
            "{label}: the probe must see the declared credential it may echo"
        );
        assert_completed_natively(&label, &observed);
        assert!(
            !inspection.turn_arguments().contains(BYPASS),
            "{label}: unavailable inspection enabled the automatic bypass"
        );
        let diagnostics = observed.fallback_diagnostics();
        assert_eq!(
            diagnostics.len(),
            1,
            "{label}: expected one visible permission-inspection fallback diagnostic, got {:?}",
            observed.records
        );
        assert_safe_diagnostic(&label, diagnostics[0]);
        causes.insert(label, diagnostics[0].text.clone());
    }

    // The version check runs once per adapter. A CLI removed after it leaves the next probe unable
    // to start; the native launch failure stays authoritative and the fallback is still reported.
    let inspection = Inspection::new(Probe::Unset).await;
    assert_completed_natively("SpawnFailure setup", &inspection.observe(1, false).await);
    fs::remove_file(&inspection.executable).assert_value();
    let observed = inspection.observe(2, false).await;
    assert!(observed.completion.is_err(), "{:?}", observed.completion);
    let diagnostics = observed.fallback_diagnostics();
    assert!(
        !diagnostics.is_empty(),
        "SpawnFailure: expected a visible permission-inspection fallback diagnostic, got {:?}",
        observed.records
    );
    for diagnostic in &diagnostics {
        assert_safe_diagnostic("SpawnFailure", diagnostic);
    }
    causes.insert("SpawnFailure".to_owned(), diagnostics[0].text.clone());

    // Each reported category names its own cause; exchange failures may share early-exit wording
    // and incomplete answers may share malformed-response wording.
    let distinct = [
        "SpawnFailure",
        "EarlyExit",
        "NativeError",
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
async fn answered_claude_permission_inspection_reports_no_fallback() {
    for case in [Probe::Unset, Probe::Configured] {
        let label = format!("{case:?}");
        let inspection = Inspection::new(case).await;
        let observed = inspection.observe(1, false).await;
        assert_completed_natively(&label, &observed);
        assert_eq!(
            inspection.turn_arguments().contains(BYPASS),
            matches!(case, Probe::Unset),
            "{label}: only verified unset policy receives the automatic bypass"
        );
        assert!(
            observed
                .records
                .iter()
                .filter(|record| record.stream != LiveOutputStream::System)
                .all(|record| !record.text.to_lowercase().contains("permission")),
            "{label}: answered inspection reported a false fallback: {:?}",
            observed.records
        );
    }
}

#[tokio::test]
async fn cancelled_claude_permission_inspection_never_reports_a_fallback() {
    let inspection = Inspection::new(Probe::Timeout).await;
    let mut handle = inspection
        .runner
        .start(request(
            inspection.binding.clone(),
            1,
            &[(ANTHROPIC_KEY, CREDENTIAL)],
        ))
        .await
        .assert_value();
    let mut output = handle.take_initial_output().assert_value();
    wait_for_file(&inspection.workspace.child("probe-child-pid")).await;
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
    assert!(!inspection.workspace.child("turn-arguments").exists());
}
