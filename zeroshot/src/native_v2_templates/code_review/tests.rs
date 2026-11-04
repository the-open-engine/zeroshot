use super::super::review::tests::{reduce_with_input, settle};
use super::*;
use openengine_cluster_protocol::{
    IdempotencyKey, NodeRuntimeBinding, RunSubmission, RunTitle, RuntimePlan, SessionScope,
    TerminalResult, WorkerErrorCode, WorkerOutcome,
};
use openengine_cluster_server::admission::VerifiedGraph;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use crate::full_v1_reducer::{Decision, DurableExecution, Reduction};
use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};

const URL: &str = "https://github.com/acme/project/pull/42";
const INSTRUCTIONS: &str = "Focus on persistence.";
const SHARED_FILE: &str = "src/shared.rs";

fn input() -> Value {
    json!({"url":URL,"instructions":INSTRUCTIONS,"context":{
        "title":"Fix stale writes","body":"Closes #7",
        "reviewScope":{
            "baseRepository":"acme/project","baseBranch":"main","baseRevision":"b".repeat(40),
            "headRepository":"contributor/project","headBranch":"fix","headRevision":"a".repeat(40)
        },
        "linkedIssues":[{"url":"https://github.com/acme/project/issues/7","title":"Prevent stale writes","body":"Reject an outdated version."}]
    }})
}

async fn graph(delivery: TemplateDelivery) -> VerifiedGraph {
    let mut runtime: RuntimePlan = serde_json::from_str(include_str!(
        "../../../../docs/assets/code-review/runtime.json"
    ))
    .assert_value();
    if delivery == TemplateDelivery::None {
        runtime
            .nodes_mut()
            .remove(&node_name(DELIVERY_NODE).assert_value());
    }
    for (name, binding) in runtime.nodes() {
        if name.as_str() != DELIVERY_NODE {
            assert!(
                matches!(
                    binding,
                    NodeRuntimeBinding::Agent {
                        session_scope: SessionScope::Execution,
                        ..
                    }
                ),
                "{name}"
            );
        }
    }
    let admitted = NativeV2Admission
        .admit_with_policy(
            RunSubmission {
                title: RunTitle::new("Code review").assert_value(),
                graph: code_review_graph(delivery).assert_value(),
                initial_input: input(),
                runtime,
                source: Some(
                    serde_json::from_value(json!({
                        "repository":"acme/project","branch":"main","revision":"a".repeat(40)
                    }))
                    .assert_value(),
                ),
                environment: None,
                submission_key: IdempotencyKey::new("code-review-test").assert_value(),
            },
            DeliveryPolicy::Optional,
        )
        .await
        .assert_value();
    VerifiedGraph {
        compiled_ir: admitted.graph,
        diagnostics: Vec::new(),
    }
}

fn verified(output: Value) -> WorkerOutcome {
    WorkerOutcome::Verifier {
        output,
        artifacts: Vec::new(),
        signals: BTreeMap::new(),
        diagnostic: Value::Null,
    }
}

fn finding(file: &str, title: &str) -> Value {
    json!({
        "file":file,"startLine":3,"endLine":4,"severity":"medium","title":title,
        "body":"An outdated version overwrites a newer row; compare versions before writing."
    })
}

fn units(count: usize) -> Value {
    (0..count)
        .map(|index| {
            json!({"name":format!("unit {index}"),"files":[format!("src/{index}.rs")],"focus":"Check version checks."})
        })
        .collect()
}

fn triaged(count: usize) -> WorkerOutcome {
    verified(json!({"summary":"Rejects stale writes.","units":units(count)}))
}

/// A reviewer's findings, filed against its unit's first file or the shared file.
fn reviewed(node: &str, input: &Value) -> WorkerOutcome {
    let file = input["unitFiles"][0].as_str().unwrap_or(SHARED_FILE);
    verified(json!({"findings":[finding(file, node)]}))
}

fn discovery() -> Vec<&'static str> {
    UNIT_REVIEWERS
        .iter()
        .map(|(node, _)| *node)
        .chain(HUNTERS.map(|(node, _)| node))
        .collect()
}

fn dispatched(reduction: &Reduction) -> Vec<&str> {
    let mut nodes = reduction
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Dispatch { occurrence, .. } => Some(occurrence.node.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    nodes.sort_unstable();
    nodes
}

/// One simulated run: the admitted graph, its initial input and the settled history.
struct Run<'a> {
    graph: &'a VerifiedGraph,
    initial: Value,
    history: Vec<DurableExecution>,
}

impl<'a> Run<'a> {
    fn new(graph: &'a VerifiedGraph, initial: Value) -> Self {
        Self {
            graph,
            initial,
            history: Vec::new(),
        }
    }

    fn reduce(&self) -> Reduction {
        reduce_with_input(self.graph, &self.history, &self.initial)
    }

    /// Settles dispatches of `stage` until none is pending and returns each settled input.
    fn settle_stage(
        &mut self,
        stage: &[&str],
        mut outcome: impl FnMut(&str, &Value) -> WorkerOutcome,
    ) -> Vec<(String, Value)> {
        let mut settled = Vec::new();
        loop {
            let reduction = self.reduce();
            let Some(action) = reduction.decisions.iter().find(|decision| {
                matches!(decision, Decision::Dispatch { occurrence, .. } if stage.contains(&occurrence.node.as_str()))
            }) else {
                return settled;
            };
            let Decision::Dispatch {
                occurrence, input, ..
            } = action
            else {
                unreachable!()
            };
            let node = occurrence.node.as_str().to_owned();
            let result = outcome(&node, input);
            settled.push((node, input.clone()));
            let position = self.history.len() as u64 * 2;
            self.history.push(settle(action, position, result));
        }
    }

    fn advance(&mut self, node: &str, outcome: WorkerOutcome) -> Value {
        let mut outcome = Some(outcome);
        let settled = self.settle_stage(&[node], |_, _| outcome.take().assert_value());
        assert_eq!(settled.len(), 1, "{node}");
        settled.into_iter().next().assert_value().1
    }

    /// Triage with `units` units, then every reviewer with its findings.
    fn review(&mut self, units: usize) -> Vec<(String, Value)> {
        self.advance(TRIAGE, triaged(units));
        self.settle_stage(&discovery(), reviewed)
    }

    fn finalize(&mut self) {
        self.settle_stage(&[FINALIZE_UNIT, FINALIZE_REST], |_, input| {
            reviewed("final", input)
        });
    }
}

fn assert_failed(reduction: &Reduction, reason: &str) {
    assert!(
        matches!(
            &reduction.terminal,
            Some(TerminalResult::Failed { reason: label, .. }) if label.as_str() == reason
        ),
        "{reason}: {:?}",
        reduction.terminal
    );
    assert!(dispatched(reduction).is_empty());
}

fn receipt() -> Value {
    json!({"receipt":{
        "version":"v1","mode":"comment","outcome":"posted","destinationUrl":URL,
        "repository":"acme/project","discussionKind":"pull_request","discussionId":"42",
        "commentId":"23","url":format!("{URL}#issuecomment-23"),"bodyHash":"c".repeat(64),
        "sourceRevision":"a".repeat(40)
    }})
}

fn posted() -> WorkerOutcome {
    WorkerOutcome::Verifier {
        output: receipt(),
        artifacts: Vec::new(),
        signals: BTreeMap::from([(
            field_name("delivery").assert_value(),
            enum_label("posted").assert_value(),
        )]),
        diagnostic: json!({"message":"Confirmed"}),
    }
}

#[tokio::test]
async fn reviewers_finalizers_and_report_receive_their_exact_inputs() {
    let context = input()["context"].clone();
    for delivery in [TemplateDelivery::None, TemplateDelivery::Comment] {
        let graph = graph(delivery).await;
        let mut run = Run::new(&graph, input());
        let triage_input = run.advance(TRIAGE, triaged(2));
        assert_eq!(
            triage_input,
            json!({"url":URL,"context":context,"instructions":INSTRUCTIONS})
        );

        let mut expected = discovery();
        expected.extend(UNIT_REVIEWERS.iter().map(|(node, _)| *node));
        expected.sort_unstable();
        assert_eq!(dispatched(&run.reduce()), expected);
        let reviews = run.settle_stage(&discovery(), reviewed);
        assert_eq!(reviews.len(), expected.len());
        for (node, review) in &reviews {
            assert_eq!(review["context"], context, "{node}");
            assert_eq!(review["instructions"], INSTRUCTIONS, "{node}");
            assert_eq!(review[SUMMARY], "Rejects stale writes.", "{node}");
            if HUNTERS.iter().any(|(hunter, _)| *hunter == node) {
                assert_eq!(review[UNITS], units(2), "{node}");
            } else {
                let index = review["unitName"]
                    .as_str()
                    .and_then(|name| name.strip_prefix("unit "))
                    .assert_value();
                assert_eq!(review["unitFiles"], json!([format!("src/{index}.rs")]));
                assert_eq!(review["unitFocus"], "Check version checks.");
            }
        }

        assert_eq!(
            dispatched(&run.reduce()),
            [FINALIZE_REST, FINALIZE_UNIT, FINALIZE_UNIT]
        );
        let finalized = run.settle_stage(&[FINALIZE_UNIT, FINALIZE_REST], |_, input| {
            reviewed("final", input)
        });
        for (node, finalizer) in &finalized {
            for (reviewer, _) in UNIT_REVIEWERS {
                assert_eq!(
                    finalizer[findings_field(reviewer)],
                    json!([
                        [finding("src/0.rs", reviewer)],
                        [finding("src/1.rs", reviewer)]
                    ]),
                    "{node}"
                );
            }
            for (hunter, _) in HUNTERS {
                assert_eq!(
                    finalizer[findings_field(hunter)],
                    json!([finding(SHARED_FILE, hunter)]),
                    "{node}"
                );
            }
            assert_eq!(finalizer["instructions"], INSTRUCTIONS);
            if node == FINALIZE_UNIT {
                assert!(finalizer["unitName"].is_string());
                assert!(finalizer.get("unitFocus").is_none());
            } else {
                assert_eq!(finalizer[UNITS], units(2));
            }
        }

        let body =
            "1 medium finding.\n\n- **medium** `src/0.rs:3-4` Stale versions overwrite rows.";
        let report_input = run.advance(REPORT, verified(json!({"body":body})));
        assert_eq!(
            report_input[UNIT_FINAL],
            json!([
                [finding("src/0.rs", "final")],
                [finding("src/1.rs", "final")]
            ])
        );
        assert_eq!(
            report_input[REST_FINAL],
            json!([finding(SHARED_FILE, "final")])
        );
        assert_eq!(report_input["context"], context);
        assert_eq!(report_input["instructions"], INSTRUCTIONS);

        let output = if delivery == TemplateDelivery::None {
            json!({"body":body})
        } else {
            let published = run.advance(DELIVERY_NODE, posted());
            assert_eq!(
                published,
                json!({"url":URL,"body":body,"reviewScope":context["reviewScope"]})
            );
            receipt()
        };
        assert_eq!(
            run.reduce().terminal,
            Some(TerminalResult::Succeeded { output })
        );
    }
}

#[tokio::test]
async fn a_change_without_units_still_gets_the_whole_change_review() {
    let graph = graph(TemplateDelivery::Comment).await;
    let mut initial = input();
    initial
        .as_object_mut()
        .assert_value()
        .remove("instructions");
    let mut run = Run::new(&graph, initial);
    let triage_input = run.advance(TRIAGE, triaged(0));
    assert_eq!(triage_input["instructions"], "");
    let mut hunters = HUNTERS.map(|(node, _)| node).to_vec();
    hunters.sort_unstable();
    assert_eq!(dispatched(&run.reduce()), hunters);
    run.settle_stage(&discovery(), reviewed);
    assert_eq!(dispatched(&run.reduce()), [FINALIZE_REST]);
    let rest_input = run.advance(FINALIZE_REST, verified(json!({"findings":[]})));
    assert_eq!(rest_input[findings_field("unit_depth")], json!([]));
    let report_input = run.advance(REPORT, verified(json!({"body":"No actionable findings."})));
    assert_eq!(report_input[UNIT_FINAL], json!([]));
    assert_eq!(report_input[REST_FINAL], json!([]));
    assert_eq!(dispatched(&run.reduce()), [DELIVERY_NODE]);
}

#[tokio::test]
async fn the_maximum_unit_count_reaches_delivery_without_losing_findings() {
    let graph = graph(TemplateDelivery::Comment).await;
    let mut run = Run::new(&graph, input());
    let reviews = run.review(MAX_UNITS as usize);
    assert_eq!(
        reviews.len(),
        MAX_UNITS as usize * UNIT_REVIEWERS.len() + HUNTERS.len()
    );
    run.finalize();
    let report = run.advance(REPORT, verified(json!({"body":"Review complete."})));
    let finalized = report[UNIT_FINAL].as_array().assert_value();
    assert_eq!(finalized.len(), MAX_UNITS as usize);
    for (index, findings) in finalized.iter().enumerate() {
        assert_eq!(
            findings,
            &json!([finding(&format!("src/{index}.rs"), "final")])
        );
    }
    assert_eq!(dispatched(&run.reduce()), [DELIVERY_NODE]);
}

#[tokio::test]
async fn a_failure_before_the_report_publishes_nothing() {
    let graph = graph(TemplateDelivery::Comment).await;

    let mut run = Run::new(&graph, input());
    run.advance(
        TRIAGE,
        WorkerOutcome::declared_failure(WorkerErrorCode::Malformed),
    );
    assert_failed(&run.reduce(), "triage_failed");

    for (index, failing) in discovery().into_iter().enumerate() {
        let code = match index % 3 {
            0 => WorkerErrorCode::Refusal,
            1 => WorkerErrorCode::Timeout,
            _ => WorkerErrorCode::Malformed,
        };
        let name = if HUNTERS.iter().any(|(hunter, _)| *hunter == failing) {
            failing.to_owned()
        } else {
            map_name(failing)
        };
        let reason = format!("{name}_failed");
        let mut run = Run::new(&graph, input());
        run.advance(TRIAGE, triaged(2));
        let mut failure = Some(code);
        run.settle_stage(&discovery(), |node, input| {
            match failure.take_if(|_| node == failing) {
                Some(code) => WorkerOutcome::declared_failure(code),
                None => reviewed(node, input),
            }
        });
        assert_failed(&run.reduce(), &reason);
    }

    let mut run = Run::new(&graph, input());
    let reviews = run.review(9);
    assert!(
        reviews
            .iter()
            .all(|(node, _)| HUNTERS.iter().any(|(hunter, _)| *hunter == node))
    );
    assert_failed(&run.reduce(), "units_broad_a_failed");

    for (failing, reason) in [
        (FINALIZE_UNIT, "finalize_units_failed"),
        (FINALIZE_REST, "finalize_rest_failed"),
    ] {
        let mut run = Run::new(&graph, input());
        run.review(2);
        let mut failure = Some(WorkerErrorCode::Malformed);
        run.settle_stage(&[FINALIZE_UNIT, FINALIZE_REST], |node, input| match failure
            .take_if(|_| node == failing)
        {
            Some(code) => WorkerOutcome::declared_failure(code),
            None => reviewed("final", input),
        });
        assert_failed(&run.reduce(), reason);
    }

    let mut run = Run::new(&graph, input());
    run.review(1);
    run.finalize();
    run.advance(
        REPORT,
        WorkerOutcome::declared_failure(WorkerErrorCode::Refusal),
    );
    assert_failed(&run.reduce(), "report_failed");
}

#[tokio::test]
async fn a_crashed_reviewer_gets_one_retry() {
    let graph = graph(TemplateDelivery::Comment).await;
    for crashes in [1, 2] {
        let mut run = Run::new(&graph, input());
        run.advance(TRIAGE, triaged(1));
        let mut remaining = crashes;
        let reviews = run.settle_stage(&discovery(), |node, input| {
            if node == "hunt_state" && remaining > 0 {
                remaining -= 1;
                WorkerOutcome::declared_failure(WorkerErrorCode::Crash)
            } else {
                reviewed(node, input)
            }
        });
        let attempts = reviews
            .iter()
            .filter(|(node, _)| node == "hunt_state")
            .count();
        assert_eq!(attempts, 2);
        let reduction = run.reduce();
        if crashes == 1 {
            assert_eq!(dispatched(&reduction), [FINALIZE_REST, FINALIZE_UNIT]);
        } else {
            assert_failed(&reduction, "hunt_state_failed");
        }
    }
}

#[test]
fn code_review_takes_exactly_the_pr_review_input() {
    let code = code_review_graph(TemplateDelivery::Comment).assert_value();
    let pr = super::super::pr_review::pr_review_graph(TemplateDelivery::Comment).assert_value();
    assert_eq!(code.initial_input, pr.initial_input);
    code.initial_input.validate_value(&input()).assert_value();
    let issue = json!({"url":"https://github.com/acme/project/issues/7","context":{
        "title":"Issue","body":"Requirements","reviewScope":{}
    }});
    assert!(code.initial_input.validate_value(&issue).is_err());
}

#[test]
fn every_agent_has_complete_instructions_for_the_pr_review_input() {
    let graph = serde_json::to_value(code_review_graph(TemplateDelivery::Comment).assert_value())
        .assert_value();
    let mut agents = BTreeMap::new();
    let mut pending = vec![&graph];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(object) => {
                if let (Some(name), Some(text)) = (
                    object.get("name").and_then(Value::as_str),
                    object.get("instructions").and_then(Value::as_str),
                ) {
                    agents.insert(name.to_owned(), text.to_owned());
                }
                pending.extend(object.values());
            }
            Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    let mut expected = discovery();
    expected.extend([TRIAGE, FINALIZE_UNIT, FINALIZE_REST, REPORT]);
    expected.sort_unstable();
    assert_eq!(
        agents.keys().map(String::as_str).collect::<Vec<_>>(),
        expected
    );
    for (name, text) in &agents {
        assert!(!text.contains("{{") && !text.contains("}}"), "{name}");
        for retired in ["diffPath", "changedFiles", "network access"] {
            assert!(!text.contains(retired), "{name} mentions {retired}");
        }
        assert!(text.contains("`instructions`"), "{name}");
        if name != REPORT {
            assert!(text.contains("context.reviewScope.baseRevision"), "{name}");
            assert!(
                text.contains(
                    "Do not install dependencies or start applications, services or browsers"
                ),
                "{name}"
            );
        }
    }
    for finalizer in [FINALIZE_UNIT, FINALIZE_REST] {
        for (reviewer, _) in UNIT_REVIEWERS {
            let field = format!("`{}`", findings_field(reviewer));
            assert!(
                agents[finalizer].contains(&field),
                "{finalizer} omits {field}"
            );
        }
    }
}
