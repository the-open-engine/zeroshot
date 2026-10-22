use super::*;
use crate::full_v1_reducer::{DurableExecutionState, HistoryPosition};
use openengine_cluster_protocol::{SessionScope, WorkerOutcome};

struct ResearchProbe {
    verified: VerifiedGraph,
    input: Value,
    history: Vec<DurableExecution>,
    delivery: TemplateDelivery,
}

impl ResearchProbe {
    async fn new(delivery: TemplateDelivery, iterations: Option<u64>) -> Self {
        let mut input = json!({"task":"improve a task in nested/project"});
        if let Some(count) = iterations {
            input["options"] = json!({"iterations":count});
        }
        let graph = BuiltinGraphTemplate::AutoResearch
            .materialize(delivery)
            .assert_value();
        let runtime = runtime_for(
            BuiltinGraphTemplate::AutoResearch,
            delivery,
            &executable_leaves(&graph.root),
        );
        let admitted = NativeV2Admission
            .admit(RunSubmission {
                title: RunTitle::new("Research control probe").assert_value(),
                graph,
                initial_input: input.clone(),
                runtime,
                environment: None,
                source: Some(resolved_source()),
                submission_key: IdempotencyKey::new("research-control-probe").assert_value(),
            })
            .await
            .assert_value();
        Self {
            verified: VerifiedGraph {
                compiled_ir: admitted.graph,
                diagnostics: Vec::new(),
            },
            input,
            history: Vec::new(),
            delivery,
        }
    }

    fn next(&self) -> Reduction {
        reduce(&self.verified, &self.input, &self.history)
    }

    fn next_input(&self) -> Value {
        self.next()
            .decisions
            .into_iter()
            .find_map(|d| match d {
                Decision::Dispatch { input, .. } => Some(input),
                _ => None,
            })
            .assert_value()
    }

    fn count(&self, node: &str) -> usize {
        self.history
            .iter()
            .filter(|event| event.occurrence.node.as_str() == node)
            .count()
    }

    fn settle(&mut self, verdict: &str, failure: Option<(&str, WorkerErrorCode)>) {
        let reduction = self.next();
        assert!(reduction.terminal.is_none());
        let mut dispatched = 0;
        for decision in reduction.decisions {
            if let Decision::Dispatch {
                node_instance,
                execution,
                occurrence,
                attempt,
                input,
                ..
            } = decision
            {
                dispatched += 1;
                let name = occurrence.node.as_str();
                let outcome =
                    if let Some((failed, code)) = failure.filter(|(failed, _)| *failed == name) {
                        assert_eq!(failed, name);
                        WorkerOutcome::declared_failure(code)
                    } else {
                        self.outcome(name, verdict)
                    };
                let position = (self.history.len() as u64 + 1) * 2;
                self.history.push(DurableExecution {
                    dispatch_position: HistoryPosition::new(position - 1).assert_value(),
                    node_instance,
                    execution,
                    occurrence,
                    attempt,
                    input,
                    state: DurableExecutionState::Settled {
                        position: HistoryPosition::new(position).assert_value(),
                        outcome,
                    },
                });
            }
        }
        assert!(dispatched > 0, "graph must dispatch work or terminate");
    }

    fn outcome(&self, name: &str, verdict: &str) -> WorkerOutcome {
        let output = match name {
            "researcher" => {
                return WorkerOutcome::Verified {
                    output: json!({"researchReport":"completed experiment; exact retained hash and raw evidence paths"}),
                    artifacts: Vec::new(),
                };
            }
            "reviewer" => {
                let mut output = json!({"reviewFeedback":"round1; exact retained hash; inspect evidence; optional archive warning"});
                if self.delivery == TemplateDelivery::Push {
                    output["title"] = json!("perf: improve the measured objective");
                    output["description"] = json!("Retain the independently supported result.");
                }
                return verifier_outcome(output, "verdict", verdict);
            }
            "checkpoint_delivery" => {
                return verifier_outcome(
                    delivery_receipt(DeliveryMode::Push, verdict),
                    DELIVERY_SIGNAL_FIELD,
                    verdict,
                );
            }
            role => {
                json!({format!("{role}Proposal"):format!("{role} independently proposes a bounded test")})
            }
        };
        WorkerOutcome::Verifier {
            output,
            signals: BTreeMap::new(),
            diagnostic: json!({"message":"read-only scout"}),
            artifacts: Vec::new(),
        }
    }

    fn successful_round(&mut self) {
        assert_dispatched_together(&self.next(), &["explorer", "synthesizer", "challenger"]);
        self.settle("accepted", None);
        assert_dispatched_together(&self.next(), &["researcher"]);
        self.settle("accepted", None);
        assert_dispatched_together(&self.next(), &["reviewer"]);
        self.settle("accepted", None);
        if self.delivery == TemplateDelivery::Push {
            assert_dispatched_together(&self.next(), &["checkpoint_delivery"]);
            self.settle(DELIVERY_PUSHED_LABEL, None);
        }
    }
}

fn verifier_outcome(output: Value, field: &str, signal: &str) -> WorkerOutcome {
    WorkerOutcome::Verifier {
        output,
        signals: BTreeMap::from([(
            field_name(field).assert_value(),
            enum_label(signal).assert_value(),
        )]),
        diagnostic: json!({"message":"specific supporting evidence or corrections"}),
        artifacts: Vec::new(),
    }
}

#[tokio::test]
async fn research_runs_exact_default_and_requested_round_counts() {
    for (delivery, iterations, count) in [
        (TemplateDelivery::None, None, 10),
        (TemplateDelivery::None, Some(3), 3),
        (TemplateDelivery::Push, Some(3), 3),
    ] {
        let mut probe = ResearchProbe::new(delivery, iterations).await;
        for _ in 0..count {
            probe.successful_round();
        }
        let expected = if delivery == TemplateDelivery::Push {
            delivery_receipt(DeliveryMode::Push, DELIVERY_PUSHED_LABEL)
        } else {
            Value::Null
        };
        assert_eq!(
            probe.next().terminal,
            Some(TerminalProjection::Succeeded { output: expected })
        );
        for role in [
            "explorer",
            "synthesizer",
            "challenger",
            "researcher",
            "reviewer",
        ] {
            assert_eq!(probe.count(role), count);
            let identities = probe
                .history
                .iter()
                .filter(|e| e.occurrence.node.as_str() == role)
                .map(|e| e.node_instance)
                .collect::<BTreeSet<_>>();
            assert_eq!(identities.len(), 1, "role identities must survive rounds");
        }
    }
}

#[tokio::test]
async fn research_binds_large_counts_without_a_template_specific_cap() {
    let probe = ResearchProbe::new(TemplateDelivery::None, Some(200)).await;
    let GraphNode::Seq(root) = &probe.verified.compiled_ir.root else {
        panic!("research root must be a sequence");
    };
    let GraphNode::Loop(research) = root.children.as_slice().first().assert_value() else {
        panic!("research loop must be first");
    };
    assert_eq!(research.max_iterations.get(), 200);
}

#[tokio::test]
async fn research_passes_native_acceptance_with_unprefixed_advisory_feedback() {
    for delivery in [TemplateDelivery::None, TemplateDelivery::Push] {
        let mut probe = ResearchProbe::new(delivery, Some(2)).await;
        probe.settle("accepted", None); // first scouts
        let first = probe.next_input();
        assert_eq!(first["review"], json!({}));
        probe.settle("accepted", None); // researcher
        probe.settle("accepted", None); // reviewer
        if delivery == TemplateDelivery::Push {
            probe.settle(DELIVERY_PUSHED_LABEL, None);
        }
        probe.settle("accepted", None); // second scouts
        let next = probe.next_input();
        assert_eq!(next["review"]["verdict"], "accepted");
        assert_eq!(
            next["reviewFeedback"],
            "round1; exact retained hash; inspect evidence; optional archive warning"
        );
    }
}

#[tokio::test]
async fn research_delivery_error_stops_before_another_round() {
    let mut probe = ResearchProbe::new(TemplateDelivery::Push, Some(2)).await;
    for _ in 0..3 {
        probe.settle("accepted", None);
    }
    probe.settle(
        "accepted",
        Some(("checkpoint_delivery", WorkerErrorCode::Crash)),
    );
    assert!(
        matches!(probe.next().terminal, Some(TerminalProjection::Failed { reason, .. }) if reason.as_str() == "research_delivery_failed")
    );
    assert_eq!(probe.count("explorer"), 1);
    assert_eq!(probe.count("checkpoint_delivery"), 1);
}

#[tokio::test]
async fn research_revisions_preserve_feedback_and_parent_scouts_without_spending_a_round() {
    for delivery in [TemplateDelivery::None, TemplateDelivery::Push] {
        let mut probe = ResearchProbe::new(delivery, Some(2)).await;
        probe.settle("accepted", None); // first scouts
        for _ in 0..3 {
            probe.settle("accepted", None); // researcher
            probe.settle("revise", None); // fresh reviewer rejects
            let next = probe.next();
            assert_dispatched_together(&next, &["researcher"]);
            let input = probe.next_input();
            assert_eq!(input["review"]["verdict"], "revise");
            assert_eq!(
                input["reviewFeedback"],
                "round1; exact retained hash; inspect evidence; optional archive warning"
            );
            assert!(
                input["explorerProposal"]
                    .as_str()
                    .assert_value()
                    .contains("explorer")
            );
            assert_eq!(probe.count("explorer"), 1);
            assert_eq!(probe.count("checkpoint_delivery"), 0);
        }
        probe.settle("accepted", None);
        probe.settle("accepted", None);
        if delivery == TemplateDelivery::Push {
            probe.settle(DELIVERY_PUSHED_LABEL, None);
        }
        probe.successful_round();
        assert!(matches!(
            probe.next().terminal,
            Some(TerminalProjection::Succeeded { .. })
        ));
        assert_eq!(probe.count("researcher"), 5);
        assert_eq!(probe.count("explorer"), 2);
    }
}

#[tokio::test]
async fn research_never_reuses_a_previous_rounds_approval() {
    let mut probe = ResearchProbe::new(TemplateDelivery::Push, Some(2)).await;
    probe.successful_round();
    probe.settle("accepted", None); // next scouts
    probe.settle("accepted", None); // next researcher
    assert_dispatched_together(&probe.next(), &["reviewer"]);
    assert_eq!(probe.count("checkpoint_delivery"), 1);
    probe.settle("revise", None);
    assert_dispatched_together(&probe.next(), &["researcher"]);
    assert_eq!(probe.count("checkpoint_delivery"), 1);
}

#[tokio::test]
async fn research_stop_requires_review_and_publishes_only_the_reviewed_result() {
    for delivery in [TemplateDelivery::None, TemplateDelivery::Push] {
        let mut probe = ResearchProbe::new(delivery, Some(50)).await;
        probe.settle("accepted", None);
        probe.settle("accepted", None);
        assert!(probe.next().terminal.is_none());
        probe.settle("stop", None);
        if delivery == TemplateDelivery::Push {
            probe.settle(DELIVERY_PUSHED_LABEL, None);
        }
        assert!(matches!(
            probe.next().terminal,
            Some(TerminalProjection::Succeeded { .. })
        ));
        assert_eq!(probe.count("explorer"), 1);
    }
}

#[tokio::test]
async fn research_exhausted_review_never_scouts_or_publishes_a_new_round() {
    for delivery in [TemplateDelivery::None, TemplateDelivery::Push] {
        let mut probe = ResearchProbe::new(delivery, Some(2)).await;
        probe.settle("accepted", None);
        for _ in 0..64 {
            probe.settle("accepted", None);
            probe.settle("revise", None);
        }
        assert!(
            matches!(probe.next().terminal, Some(TerminalProjection::Failed { reason, .. }) if reason.as_str() == "research_review_budget_exhausted")
        );
        assert_eq!(probe.count("explorer"), 1);
        assert_eq!(probe.count("checkpoint_delivery"), 0);
    }
}

#[tokio::test]
async fn research_execution_errors_fail_closed_before_publication() {
    for role in ["explorer", "researcher", "reviewer"] {
        let mut probe = ResearchProbe::new(TemplateDelivery::Push, Some(2)).await;
        if role != "explorer" {
            probe.settle("accepted", None);
        }
        if role == "reviewer" {
            probe.settle("accepted", None);
        }
        probe.settle("accepted", Some((role, WorkerErrorCode::Crash)));
        if role != "researcher" {
            probe.settle("accepted", Some((role, WorkerErrorCode::Crash)));
        }
        assert!(matches!(
            probe.next().terminal,
            Some(TerminalProjection::Failed { .. })
        ));
        assert_eq!(probe.count("checkpoint_delivery"), 0);
    }
}

#[tokio::test]
async fn research_delivery_repair_is_explicit_and_never_authorizes_an_unreviewed_writer() {
    let mut probe = ResearchProbe::new(TemplateDelivery::Push, Some(2)).await;
    for _ in 0..3 {
        probe.settle("accepted", None);
    }
    probe.settle(DELIVERY_REPAIR_REQUIRED_LABEL, None);
    assert!(
        matches!(probe.next().terminal, Some(TerminalProjection::Failed { reason, .. }) if reason.as_str() == "research_delivery_requires_reconciliation")
    );
    assert_eq!(probe.count("researcher"), 1);
}

#[tokio::test]
async fn research_rejects_invalid_iteration_limits_and_unsupported_delivery() {
    for count in [0, 9_007_199_254_740_992_u64] {
        let graph = BuiltinGraphTemplate::AutoResearch
            .materialize(TemplateDelivery::None)
            .assert_value();
        let runtime = runtime_for(
            BuiltinGraphTemplate::AutoResearch,
            TemplateDelivery::None,
            &executable_leaves(&graph.root),
        );
        let error = NativeV2Admission
            .admit(RunSubmission {
                title: RunTitle::new("Invalid count").assert_value(),
                graph,
                initial_input: json!({"task":"research","options":{"iterations":count}}),
                runtime,
                environment: None,
                source: Some(resolved_source()),
                submission_key: IdempotencyKey::new("invalid-research-count").assert_value(),
            })
            .await
            .assert_error();
        assert!(error.to_string().contains("positive safe integer"));
    }
    for delivery in [TemplateDelivery::PullRequest, TemplateDelivery::Merge] {
        assert!(
            BuiltinGraphTemplate::AutoResearch
                .materialize(delivery)
                .is_err()
        );
    }
}

#[test]
fn research_has_five_model_roles_and_consistent_workspace_root_guidance() {
    let graph = BuiltinGraphTemplate::AutoResearch
        .materialize(TemplateDelivery::None)
        .assert_value();
    let nodes = executable_leaves(&graph.root);
    assert_eq!(nodes.len(), 5);
    for node in nodes {
        let text = match node {
            GraphNode::Step(n) => n.instructions.as_ref(),
            GraphNode::Verifier(n) => n.instructions.as_ref(),
            _ => None,
        }
        .assert_value()
        .as_str();
        assert!(text.contains("workspace root") || text.contains("workspace-root"));
        let expected = if node.name().as_str() == "reviewer" {
            SessionScope::Execution
        } else {
            SessionScope::NodeInstance
        };
        assert_eq!(
            BuiltinGraphTemplate::AutoResearch.default_session_scope(node.name().as_str()),
            expected
        );
    }
}
