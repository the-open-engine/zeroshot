//! Claude Code 2.1.310+ final results and per-invocation usage.
//!
//! The stub prints record shapes from Claude Code's stream-json result schema (2.1.310 and
//! 2.1.318). One `--print` process writes one result per main-loop turn while background work
//! runs: `result_index` rises and later results come from task notifications. The last result
//! before stdout EOF is authoritative. `modelUsage` is the session's running total, which carries
//! earlier processes on `--resume`. `model_usage_since_last_result` states each call once, `{}`
//! means nothing was counted, and absent is not `{}`. `usage` covers only the main thread.

use super::*;
use crate::native_v2_contract::NodeCompletion;
use crate::native_v2_runner::{DurableNodeEvent, NodeHandle};

const MODEL: &str = "claude-sonnet-5";
const CORRECTION_PROMPT: &str = "Your previous final response was rejected mechanically";

type Counts = [u64; 4];

enum Answer<'a> {
    Structured(&'a str),
    Prose(&'a str),
}

fn model_usage(counts: Counts) -> Value {
    json!({
        MODEL: {
            "inputTokens": counts[0],
            "outputTokens": counts[1],
            "cacheReadInputTokens": counts[2],
            "cacheCreationInputTokens": counts[3],
            "webSearchRequests": 0,
            "costUSD": 0.01,
            "contextWindow": 200000,
            "maxOutputTokens": 64000
        }
    })
}

fn main_thread_usage(counts: Counts) -> Value {
    json!({
        "input_tokens": counts[0],
        "output_tokens": counts[1],
        "cache_read_input_tokens": counts[2],
        "cache_creation_input_tokens": counts[3]
    })
}

fn subagent_stats(spawned: u64, completed: u64, killed_by_cli: u64) -> Value {
    json!({
        "spawned": spawned,
        "requested": {"background": spawned, "foreground": 0, "unset": 0},
        "started_in_background": spawned,
        "by_type": if spawned == 0 { json!({}) } else { json!({"general-purpose": spawned}) },
        "max_depth": u64::from(spawned > 0),
        "spawned_by_subagents": 0,
        "completed": completed,
        "failed": 0,
        "killed": {"parent": 0, "user": 0, "system": killed_by_cli},
        "refused": {"depth_limit": 0, "concurrency_limit": 0, "budget": 0}
    })
}

/// One numbered result record. `since` is `model_usage_since_last_result`.
fn result(index: u64, main: Counts, total: Counts, since: Option<Value>) -> Value {
    let mut record = json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "duration_ms": 1200,
        "duration_api_ms": 900,
        "num_turns": 1,
        "stop_reason": "end_turn",
        "session_id": "session-1",
        "total_cost_usd": 0.01,
        "usage": main_thread_usage(main),
        "modelUsage": model_usage(total),
        "permission_denials": [],
        "result_index": index,
        "uuid": format!("result-{index}")
    });
    let fields = record.as_object_mut().assert_value();
    if index > 0 {
        fields.insert("origin".to_owned(), json!({"kind": "task-notification"}));
    }
    if let Some(since) = since {
        fields.insert("model_usage_since_last_result".to_owned(), since);
    }
    record
}

fn answered(answer: Answer<'_>, mut record: Value) -> Value {
    let fields = record.as_object_mut().assert_value();
    match answer {
        Answer::Structured(text) => {
            let structured = json!({"response": text});
            fields.insert("result".to_owned(), json!(structured.to_string()));
            fields.insert("structured_output".to_owned(), structured);
        }
        Answer::Prose(text) => {
            fields.insert("result".to_owned(), json!(text));
        }
    }
    record
}

fn with_subagents(mut record: Value, stats: Value) -> Value {
    record
        .as_object_mut()
        .assert_value()
        .insert("subagent_stats".to_owned(), stats);
    record
}

fn execution_error(index: u64, main: Counts, total: Counts, since: Counts) -> Value {
    let mut record = result(index, main, total, Some(model_usage(since)));
    let fields = record.as_object_mut().assert_value();
    fields.insert("subtype".to_owned(), json!("error_during_execution"));
    fields.insert("is_error".to_owned(), json!(true));
    fields.insert("stop_reason".to_owned(), Value::Null);
    fields.insert(
        "errors".to_owned(),
        json!(["API Error: 500 Internal server error"]),
    );
    record
}

/// Prints whole records in one write.
fn print(records: &[Value]) -> String {
    let mut shell = String::new();
    for record in records {
        shell.push_str(&format!("printf '%s\\n' '{record}'\n"));
    }
    shell
}

/// Prints one record in two writes, with a pause between them.
fn print_split(record: &Value) -> String {
    let text = record.to_string();
    let (first, second) = text.split_at(text.len() / 2);
    format!("printf '%s' '{first}'\nsleep 0.2\nprintf '%s\\n' '{second}'\n")
}

fn stderr(line: &str) -> String {
    format!("printf '%s\\n' '{line}' >&2\n")
}

/// A Claude Code stub that reports `version`, records each model turn, and resumes `session-1`.
fn stub(version: &str, initial: &str, resumed: &str) -> String {
    format!(
        r#"set -eu
for argument in "$@"; do
  case "$argument" in
    --version|-v) printf '%s\n' '{version} (Claude Code)'; exit 0 ;;
  esac
done
target=initial
previous=
for argument in "$@"; do
  if [ "$previous" = "--resume" ]; then target=resumed; fi
  previous=$argument
done
printf '%s\n' "$@" > "$target.args"
cat > "$target.prompt"
printf '%s\n' "$target" >> turns.log
printf '%s\n' '{{"type":"system","subtype":"init","session_id":"session-1","claude_code_version":"{version}","model":"{MODEL}"}}'
case "$target" in
initial)
{initial}
;;
resumed)
{resumed}
;;
esac
"#
    )
}

struct Finished {
    logs: String,
    usages: Vec<Option<Counts>>,
    completion: Result<NodeCompletion, NodeRunnerError>,
}

impl Finished {
    fn answer(&self) -> Option<&Value> {
        match &self.completion {
            Ok(NodeCompletion {
                outcome: WorkerOutcome::Verified { output, .. },
                ..
            }) => Some(output),
            _ => None,
        }
    }

    fn assert_answer(&self, expected: &str) {
        assert_eq!(
            self.answer(),
            Some(&json!(expected)),
            "unexpected completion: {:?}",
            self.completion
        );
    }

    fn assert_not_verified(&self) {
        assert!(
            self.answer().is_none(),
            "an unfinished Claude invocation was verified: {:?}",
            self.completion
        );
        assert!(!matches!(self.completion, Err(NodeRunnerError::Cancelled)));
    }

    fn failure_text(&self) -> String {
        let error = self
            .completion
            .as_ref()
            .err()
            .map(ToString::to_string)
            .unwrap_or_default();
        format!("{}\n{error}", self.logs)
    }
}

fn counts(usage: Option<TokenUsageDelta>) -> Option<Counts> {
    usage.map(|usage| {
        [
            usage.input_tokens.get(),
            usage.output_tokens.get(),
            usage.cache_read_input_tokens.assert_value().get(),
            usage.cache_creation_input_tokens.assert_value().get(),
        ]
    })
}

async fn finish(mut handle: NodeHandle) -> Finished {
    let mut durable = handle.take_initial_output().assert_value();
    let collect = async {
        let mut logs = Vec::new();
        let mut usages = Vec::new();
        while let Ok(event) = durable.recv().await {
            match event {
                DurableNodeEvent::Output { output, .. } => logs.push(output.text),
                DurableNodeEvent::TokenUsage(usage) => usages.push(counts(usage)),
            }
        }
        (logs, usages)
    };
    let ((logs, usages), completion) = tokio::join!(collect, handle.completion());
    Finished {
        logs: logs.join("\n"),
        usages,
        completion,
    }
}

async fn claude_runner(
    workspace: &TestDirectory,
    scope: SessionScope,
) -> (NativeNodeRunner, NodeRuntimeBinding) {
    let binding = agent_binding(MODEL, Some(ReasoningEffort::High), scope, &[ANTHROPIC_KEY]);
    let runtime = runner(workspace, ClaudeProvider::Anthropic, binding.clone(), false).await;
    (runtime, binding)
}

async fn execute(
    runtime: &NativeNodeRunner,
    binding: &NodeRuntimeBinding,
    execution: u64,
) -> Finished {
    let handle = runtime
        .start(request(
            binding.clone(),
            execution,
            &[(ANTHROPIC_KEY, "anthropic-fake")],
        ))
        .await
        .assert_value();
    finish(handle).await
}

async fn run_once(label: &str, script: &str) -> (TestDirectory, Finished) {
    let workspace = TestDirectory::new(label);
    workspace.write("fake-claude.sh", script);
    let (runtime, binding) = claude_runner(&workspace, SessionScope::Execution).await;
    let finished = execute(&runtime, &binding, 1).await;
    (workspace, finished)
}

fn turns(workspace: &TestDirectory) -> Vec<String> {
    if !workspace.child("turns.log").exists() {
        return Vec::new();
    }
    workspace
        .read("turns.log")
        .lines()
        .map(str::to_owned)
        .collect()
}

fn early_shell_result() -> Value {
    answered(
        Answer::Structured("early"),
        result(
            0,
            [30, 10, 100, 20],
            [30, 10, 100, 20],
            Some(model_usage([30, 10, 100, 20])),
        ),
    )
}

fn shell_follow_up(answer: Answer<'_>) -> Value {
    answered(
        answer,
        result(
            1,
            [30, 5, 150, 10],
            [60, 15, 250, 30],
            Some(model_usage([30, 5, 150, 10])),
        ),
    )
}

#[tokio::test]
async fn background_follow_up_result_replaces_the_early_answer_and_adds_its_usage() {
    // A background shell finished after result 0 was written; the CLI then ran a notification turn.
    let body = format!(
        "{}sleep 0.2\n{}",
        print(&[early_shell_result()]),
        print_split(&shell_follow_up(Answer::Structured("final")))
    );
    let (_workspace, finished) =
        run_once("claude-follow-up-result", &stub("2.1.318", &body, &body)).await;
    finished.assert_answer("final");
    assert_eq!(finished.usages, [Some([60, 15, 250, 30])]);
}

#[tokio::test]
async fn held_back_results_written_together_return_the_last_answer() {
    // A background subagent made the CLI hold result 0 until its notification turn ended. Both
    // results were then written at once with the final running total; result 0 states every call.
    let stats = subagent_stats(1, 1, 0);
    let early = answered(
        Answer::Structured("early"),
        result(
            0,
            [10, 100, 1000, 500],
            [27, 270, 2700, 1300],
            Some(model_usage([27, 270, 2700, 1300])),
        ),
    );
    let last = answered(
        Answer::Structured("final"),
        result(
            1,
            [10, 100, 1000, 500],
            [27, 270, 2700, 1300],
            Some(json!({})),
        ),
    );
    let body = print(&[
        with_subagents(early, stats.clone()),
        with_subagents(last, stats),
    ]);
    let (_workspace, finished) =
        run_once("claude-held-back-results", &stub("2.1.318", &body, &body)).await;
    finished.assert_answer("final");
    assert_eq!(finished.usages, [Some([27, 270, 2700, 1300])]);
}

#[tokio::test]
async fn later_error_result_fails_instead_of_returning_the_early_answer() {
    let late_error = execution_error(1, [30, 5, 150, 10], [60, 15, 250, 30], [30, 5, 150, 10]);
    let body = print(&[early_shell_result(), late_error]);
    let (_workspace, finished) =
        run_once("claude-later-error", &stub("2.1.318", &body, &body)).await;
    finished.assert_not_verified();
    assert_eq!(finished.usages.first(), Some(&Some([60, 15, 250, 30])));
}

#[tokio::test]
async fn missing_final_structured_output_is_corrected_in_the_same_session() {
    // 2.1.318 can end a notification turn with prose after its structured-output nudge.
    let initial = print(&[
        early_shell_result(),
        shell_follow_up(Answer::Prose("The background check finished.")),
    ]);
    let corrected = print(&[answered(
        Answer::Structured("final"),
        result(
            0,
            [7, 3, 5, 1],
            [67, 18, 255, 31],
            Some(model_usage([7, 3, 5, 1])),
        ),
    )]);
    let (workspace, finished) = run_once(
        "claude-final-output-correction",
        &stub("2.1.318", &initial, &corrected),
    )
    .await;
    finished.assert_answer("final");
    assert_eq!(turns(&workspace), ["initial", "resumed"]);
    assert_resumed_session(&workspace.read("resumed.args"));
    assert!(workspace.read("resumed.prompt").contains(CORRECTION_PROMPT));
    assert_eq!(
        finished.usages,
        [Some([60, 15, 250, 30]), Some([7, 3, 5, 1])]
    );
}

fn resumed_turn(since: Option<Value>) -> String {
    print(&[answered(
        Answer::Structured("second"),
        result(0, [7, 3, 5, 1], [18, 7, 11, 3], since),
    )])
}

async fn resumed_execution_usage(label: &str, resumed: &str) -> [Vec<Option<Counts>>; 2] {
    let first = print(&[answered(
        Answer::Structured("first"),
        result(
            0,
            [11, 4, 6, 2],
            [11, 4, 6, 2],
            Some(model_usage([11, 4, 6, 2])),
        ),
    )]);
    let workspace = TestDirectory::new(label);
    workspace.write("fake-claude.sh", &stub("2.1.310", &first, resumed));
    let (runtime, binding) = claude_runner(&workspace, SessionScope::NodeInstance).await;
    let initial = execute(&runtime, &binding, 1).await;
    initial.assert_answer("first");
    let resumed = execute(&runtime, &binding, 2).await;
    resumed.assert_answer("second");
    assert_eq!(turns(&workspace), ["initial", "resumed"]);
    [initial.usages, resumed.usages]
}

#[tokio::test]
async fn resumed_execution_counts_only_its_own_usage() {
    let usages = resumed_execution_usage(
        "claude-resumed-usage",
        &resumed_turn(Some(model_usage([7, 3, 5, 1]))),
    )
    .await;
    assert_eq!(
        usages,
        [vec![Some([11, 4, 6, 2])], vec![Some([7, 3, 5, 1])]]
    );
}

#[tokio::test]
async fn resumed_result_without_per_result_counters_reports_unknown_usage() {
    // Neither the session total nor main-thread usage is this invocation's complete usage.
    let usages = resumed_execution_usage("claude-resumed-usage-unknown", &resumed_turn(None)).await;
    assert_eq!(usages, [vec![Some([11, 4, 6, 2])], vec![None]]);
}

#[tokio::test]
async fn subagents_killed_at_the_background_ceiling_are_not_verified() {
    let held = with_subagents(
        answered(
            Answer::Structured("early"),
            result(
                0,
                [10, 100, 1000, 500],
                [17, 170, 1700, 800],
                Some(model_usage([17, 170, 1700, 800])),
            ),
        ),
        subagent_stats(1, 0, 1),
    );
    let body = format!(
        "{}{}",
        stderr(
            "Background tasks still running 10m after the last turn (subagent \"background helper\"); stopping them. Set CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0 to wait indefinitely."
        ),
        print(&[held])
    );
    let (_workspace, finished) =
        run_once("claude-background-ceiling", &stub("2.1.318", &body, &body)).await;
    finished.assert_not_verified();
}

#[tokio::test]
async fn stopping_background_shells_at_the_ceiling_keeps_the_answer() {
    let only = with_subagents(
        answered(
            Answer::Structured("final"),
            result(
                0,
                [30, 10, 100, 20],
                [30, 10, 100, 20],
                Some(model_usage([30, 10, 100, 20])),
            ),
        ),
        subagent_stats(0, 0, 0),
    );
    let body = format!(
        "{}{}",
        stderr(
            "Background tasks still running 10m after the last turn (shell \"npm run dev\"); stopping them. Set CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=0 to wait indefinitely."
        ),
        print(&[only])
    );
    let (_workspace, finished) =
        run_once("claude-shell-ceiling", &stub("2.1.318", &body, &body)).await;
    finished.assert_answer("final");
    assert_eq!(finished.usages, [Some([30, 10, 100, 20])]);
}

fn final_answer() -> String {
    print(&[answered(
        Answer::Structured("final"),
        result(
            0,
            [30, 10, 100, 20],
            [30, 10, 100, 20],
            Some(model_usage([30, 10, 100, 20])),
        ),
    )])
}

#[tokio::test]
async fn claude_older_than_2_1_310_is_rejected_before_any_model_turn() {
    for version in ["2.1.309", "2.1.237"] {
        let body = final_answer();
        let (workspace, finished) = run_once(
            &format!("claude-version-{version}"),
            &stub(version, &body, &body),
        )
        .await;
        finished.assert_not_verified();
        assert!(
            turns(&workspace).is_empty(),
            "{version} started a model turn"
        );
        assert!(!finished.logs.contains("Claude Code version: "));
        let failure = finished.failure_text();
        assert!(
            failure.contains("2.1.310"),
            "{version} rejection does not name the minimum: {failure}"
        );
    }
}

#[tokio::test]
async fn claude_2_1_310_and_newer_are_accepted() {
    for version in ["2.1.310", "2.1.318", "2.1.1000", "3.0.0"] {
        let body = final_answer();
        let (workspace, finished) = run_once(
            &format!("claude-version-{version}"),
            &stub(version, &body, &body),
        )
        .await;
        finished.assert_answer("final");
        assert_eq!(turns(&workspace), ["initial"], "{version}");
        assert!(
            finished
                .logs
                .contains(&format!("Claude Code version: {version}"))
        );
    }
}

#[tokio::test]
async fn cancellation_while_background_work_runs_after_an_early_result_cancels() {
    let body = format!(
        "{}(sleep 30; printf survived > survivor.txt) &\nprintf '%s' \"$!\" > child.pid\nwait\n",
        print(&[early_shell_result()])
    );
    let workspace = TestDirectory::new("claude-cancel-after-early-result");
    workspace.write("fake-claude.sh", &stub("2.1.318", &body, &body));
    let (runtime, binding) = claude_runner(&workspace, SessionScope::Execution).await;
    let mut handle = runtime
        .start(request(binding, 1, &[(ANTHROPIC_KEY, "anthropic-fake")]))
        .await
        .assert_value();
    let _durable = handle.take_initial_output().assert_value();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !workspace.child("child.pid").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // The pid is written without a newline; wait until the write is complete.
        while workspace.read("child.pid").parse::<u32>().is_err() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .assert_value_with("background work did not start after the early result");
    let child_pid: u32 = workspace.read("child.pid").parse().assert_value();
    cancel_and_assert_reaped(handle, &workspace, child_pid).await;
}
