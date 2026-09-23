use std::collections::BTreeSet;

use openengine_cluster_protocol::{DataSelector, IdempotencyKey, RecordField, RunSubmission, RunTitle};
use openengine_cluster_server::admission::VerifiedGraph;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use crate::native_v2_admission::{DeliveryPolicy, NativeV2Admission};
use crate::native_v2_contract::RunSubmissionIntent;
use crate::native_v2_delivery::{DELIVERY_MERGED_LABEL, DELIVERY_PUSHED_LABEL, DELIVERY_READY_LABEL};
use crate::full_v1_reducer::{
    Decision, DurableExecution, FullV1Reducer, Reduction, ReductionInput, TerminalProjection,
};

use super::*;

#[path = "tests/support.rs"]
mod support;
use support::*;

#[test]
fn catalog_and_template_inputs_are_closed() {
    assert_eq!(
        BuiltinGraphTemplate::all(),
        &[
            BuiltinGraphTemplate::SingleWorker,
            BuiltinGraphTemplate::SoftwareChange,
            BuiltinGraphTemplate::AutoResearch,
        ]
    );
    assert_eq!(
        BuiltinGraphTemplate::parse("software-change"),
        Some(BuiltinGraphTemplate::SoftwareChange)
    );
    assert_eq!(
        BuiltinGraphTemplate::parse("auto-research"),
        Some(BuiltinGraphTemplate::AutoResearch)
    );
    assert_eq!(BuiltinGraphTemplate::parse("unknown"), None);
    assert!(matches!(
        BuiltinGraphTemplate::SingleWorker.materialize(TemplateDelivery::Merge),
        Err(BuiltinTemplateError::UnsupportedDelivery { .. })
    ));

    let authored = json!({"task":"repair checkout"});
    let single = BuiltinGraphTemplate::SingleWorker
        .materialize(TemplateDelivery::None)
        .assert_value();
    single
        .initial_input
        .validate_value(&authored)
        .assert_value();
    let research = BuiltinGraphTemplate::AutoResearch
        .materialize(TemplateDelivery::None)
        .assert_value();
    research
        .initial_input
        .validate_value(&authored)
        .assert_value();
    assert!(
        research
            .initial_input
            .validate_value(&json!({"task":"repair checkout","iterations":10}))
            .is_err()
    );
    let software = BuiltinGraphTemplate::SoftwareChange
        .materialize(TemplateDelivery::None)
        .assert_value();
    software
        .initial_input
        .validate_value(&authored)
        .assert_value();
    assert!(
        software
            .initial_input
            .validate_value(&json!({"task":"repair checkout","acceptanceFeedback":""}))
            .is_err()
    );
    assert!(
        software
            .initial_input
            .validate_value(&json!({"task":"repair checkout","issueNumber":"208"}))
            .is_err()
    );
    let delivered = BuiltinGraphTemplate::SoftwareChange
        .materialize(TemplateDelivery::PullRequest)
        .assert_value();
    delivered
        .initial_input
        .validate_value(&authored)
        .assert_value();
    delivered
        .initial_input
        .validate_value(&json!({"task":"repair checkout","issueNumber":"208"}))
        .assert_value();
}

#[tokio::test]
async fn every_supported_materialization_is_admissible() {
    let cases = vec![
        (
            BuiltinGraphTemplate::SingleWorker,
            TemplateDelivery::None,
            vec!["worker"],
        ),
        (
            BuiltinGraphTemplate::SoftwareChange,
            TemplateDelivery::None,
            vec!["acceptance", "code", "review_repair", "worker"],
        ),
        (
            BuiltinGraphTemplate::SoftwareChange,
            TemplateDelivery::Push,
            vec![
                "acceptance",
                "code",
                "deliver",
                "delivery_repair",
                "review_repair",
                "worker",
            ],
        ),
        (
            BuiltinGraphTemplate::SoftwareChange,
            TemplateDelivery::PullRequest,
            vec![
                "acceptance",
                "code",
                "deliver",
                "delivery_repair",
                "review_repair",
                "worker",
            ],
        ),
        (
            BuiltinGraphTemplate::SoftwareChange,
            TemplateDelivery::Merge,
            vec![
                "acceptance",
                "code",
                "deliver",
                "delivery_repair",
                "review_repair",
                "worker",
            ],
        ),
        (
            BuiltinGraphTemplate::AutoResearch,
            TemplateDelivery::None,
            auto_research_leaves(TemplateDelivery::None),
        ),
        (
            BuiltinGraphTemplate::AutoResearch,
            TemplateDelivery::Push,
            auto_research_leaves(TemplateDelivery::Push),
        ),
    ];

    for (template, delivery, expected) in cases {
        assert_admissible(template, delivery, &expected).await;
        let graph = template.materialize(delivery).assert_value();
        for node in all_nodes(&graph.root) {
            match node {
                GraphNode::Step(node) => assert!(node.timeout_ms.is_none()),
                GraphNode::Verifier(node) => assert!(node.timeout_ms.is_none()),
                _ => {}
            }
        }
    }
}

fn auto_research_leaves(delivery: TemplateDelivery) -> Vec<&'static str> {
    let mut leaves = vec![
        "abort_execution",
        "abort_scouting",
        "abort_review",
        "audit_disposition",
        "bootstrap",
        "experiment",
        "finalize_aborted",
        "finalize_adopted",
        "finalize_record_only",
        "research_judge",
        "research_scout",
        "select_hypothesis",
        "topology_validation",
    ];
    if delivery == TemplateDelivery::Push {
        leaves.extend([
            "checkpoint_audit",
            "checkpoint_delivery",
            "checkpoint_manifest",
        ]);
    }
    leaves
}

#[test]
fn auto_research_has_ten_gated_iterations_and_optional_checkpoint() {
    for (delivery, expected_deliveries) in
        [(TemplateDelivery::None, 0), (TemplateDelivery::Push, 1)]
    {
        assert_auto_research_materialization(delivery, expected_deliveries);
    }
    for delivery in [TemplateDelivery::PullRequest, TemplateDelivery::Merge] {
        assert!(matches!(
            BuiltinGraphTemplate::AutoResearch.materialize(delivery),
            Err(BuiltinTemplateError::UnsupportedDelivery { .. })
        ));
    }
}

fn assert_auto_research_materialization(delivery: TemplateDelivery, expected_deliveries: usize) {
    let graph = BuiltinGraphTemplate::AutoResearch
        .materialize(delivery)
        .assert_value();
    assert_research_bootstrap(&graph.root);
    let graph_nodes = all_nodes(&graph.root);
    assert_topology_preflight(&graph_nodes);
    let research_loop = graph_nodes
        .into_iter()
        .find_map(|node| match node {
            GraphNode::Loop(node) if node.name.as_str() == "research_loop" => Some(node),
            _ => None,
        })
        .assert_value_with("research loop");
    assert_eq!(research_loop.max_iterations.get(), 10);
    assert!(research_loop.until.is_none());

    let iteration_nodes = all_nodes(&research_loop.body);
    assert_research_selector(&iteration_nodes);
    assert_scout_topology(&iteration_nodes);
    assert_research_phase_topology(&iteration_nodes);
    assert_judge_topology(&iteration_nodes);
    assert_guarded_finalizers(&iteration_nodes);
    assert_research_checkpoint(&iteration_nodes, delivery, expected_deliveries);
    assert_research_decision(&iteration_nodes);
}

fn assert_research_bootstrap(root: &GraphNode) {
    let bootstrap = all_nodes(root)
        .into_iter()
        .find_map(|node| match node {
            GraphNode::Step(node) if node.name.as_str() == "bootstrap" => Some(node),
            _ => None,
        })
        .assert_value_with("research bootstrap");
    let PayloadType::Record { fields } = &bootstrap.output else {
        panic!("research bootstrap must return a record");
    };
    assert_eq!(fields.len(), 7);
    assert_topology_fields(fields);
    assert_eq!(
        fields
            .get(&field_name("continuationItems").assert_value())
            .map(|field| (&field.value_type, field.required)),
        Some((
            &PayloadType::Array {
                items: Box::new(PayloadType::Null)
            },
            true
        ))
    );
    assert!(!fields.contains_key(&field_name("activationItems").assert_value()));
    assert_eq!(bootstrap.write_bindings.len(), 7);
    assert!(bootstrap.instructions.as_ref().is_some_and(|value| {
        value
            .as_str()
            .contains("best supported historical findings")
            && value.as_str().contains("exactly three ordered scout roles")
            && value.as_str().contains("exactly one experiment work item")
            && value.as_str().contains("rejects missing")
    }));
}

fn assert_topology_fields(fields: &BTreeMap<FieldName, RecordField>) {
    assert_role_array_field(
        fields,
        "scoutRoles",
        &["explorer", "synthesizer", "challenger"],
    );
    assert_role_array_field(fields, "judgeRoles", &["evidence", "method", "progress"]);
    assert_role_array_field(fields, "workItems", &["experiment"]);
}

fn assert_role_array_field(fields: &BTreeMap<FieldName, RecordField>, name: &str, labels: &[&str]) {
    let field = fields
        .get(&field_name(name).assert_value())
        .assert_value_with(name);
    assert!(field.required);
    let PayloadType::Array { items } = &field.value_type else {
        panic!("{name} must be an array");
    };
    let PayloadType::Record { fields } = items.as_ref() else {
        panic!("{name} items must be records");
    };
    assert_eq!(fields.len(), 1);
    assert_required_role_field(fields, labels);
}

fn assert_required_role_field(fields: &BTreeMap<FieldName, RecordField>, labels: &[&str]) {
    assert_eq!(
        fields
            .get(&field_name("role").assert_value())
            .map(|field| (&field.value_type, field.required)),
        Some((
            &PayloadType::Enum {
                values: enum_labels(labels).assert_value()
            },
            true
        ))
    );
}

fn assert_research_phase_topology(iteration_nodes: &[&GraphNode]) {
    let phase = iteration_nodes
        .iter()
        .find_map(|node| match node {
            GraphNode::Map(node) if node.name.as_str() == "research_phase" => Some(node),
            _ => None,
        })
        .assert_value_with("research phase");
    assert_eq!(phase.max_items.get(), 1);
    assert_eq!(
        phase.over,
        DataSelector::State {
            path: field_path("workItems").assert_value()
        }
    );
    assert_eq!(phase.body.name().as_str(), "execution_stage");
}

fn assert_topology_preflight(nodes: &[&GraphNode]) {
    let validator = find_verifier(nodes, "topology_validation");
    assert_eq!(
        validator.worker.as_str(),
        "builtin.agent.research-topology@1"
    );
    let PayloadType::Record { fields } = &validator.input else {
        panic!("topology validator must have record input");
    };
    assert_eq!(fields.len(), 3);
    assert_topology_fields(fields);
    assert_eq!(
        validator
            .signals
            .get(&field_name(VERDICT_FIELD).assert_value()),
        Some(&enum_labels(&[ACCEPTED_LABEL, REJECTED_LABEL]).assert_value())
    );
    assert_eq!(validator.diagnostic, diagnostic_type().assert_value());
    assert!(validator.instructions.as_ref().is_some_and(|value| {
        value
            .as_str()
            .contains("cardinality, uniqueness, and order")
            && value.as_str().contains("gates the iteration loop")
    }));
    let result = find_choice(nodes, "topology_result");
    assert_eq!(
        result.branches.as_slice()[1].node.name().as_str(),
        "topology_validation_rejected"
    );
    assert_eq!(
        result.branches.as_slice()[2].node.name().as_str(),
        "topology_validation_accepted"
    );
    assert!(result.otherwise.is_none());
}

fn assert_research_selector(iteration_nodes: &[&GraphNode]) {
    let selector = iteration_nodes
        .iter()
        .find_map(|node| match node {
            GraphNode::Step(node) if node.name.as_str() == "select_hypothesis" => Some(node),
            _ => None,
        })
        .assert_value_with("research selector");
    assert!(selector.instructions.as_ref().is_some_and(|value| {
        value
            .as_str()
            .contains("retained workspace violates a charter invariant")
            && value
                .as_str()
                .contains("Predeclare evidence collection and evaluation order")
    }));
    assert_recovery_route(iteration_nodes, "execution_result", "abort_execution_stage");
}

fn assert_scout_topology(iteration_nodes: &[&GraphNode]) {
    let scouts = iteration_nodes
        .iter()
        .find_map(|node| match node {
            GraphNode::Map(node) if node.name.as_str() == "hypothesis_scouts" => Some(node),
            _ => None,
        })
        .assert_value_with("hypothesis scouts");
    assert_eq!(scouts.max_items.get(), 3);
    assert_eq!(
        scouts.over,
        DataSelector::State {
            path: field_path("scoutRoles").assert_value()
        }
    );
    assert_eq!(scouts.body.name().as_str(), "research_scout");
    let scout = assert_role_verifier(
        iteration_nodes,
        "research_scout",
        "builtin.agent.research-scout@1",
        &["explorer", "synthesizer", "challenger"],
    );
    assert!(scout.instructions.is_some());
    assert_recovery_route(iteration_nodes, "scout_result", "abort_scouting_stage");
}

fn assert_judge_topology(iteration_nodes: &[&GraphNode]) {
    let judge_phase = iteration_nodes
        .iter()
        .find_map(|node| match node {
            GraphNode::Map(node) if node.name.as_str() == "judge_phase" => Some(node),
            _ => None,
        })
        .assert_value_with("judge phase");
    assert_eq!(judge_phase.max_items.get(), 1);
    assert!(judge_phase.promoted_state_paths.is_empty());

    let audit_phase = iteration_nodes
        .iter()
        .find_map(|node| match node {
            GraphNode::Map(node) if node.name.as_str() == "disposition_audit_phase" => Some(node),
            _ => None,
        })
        .assert_value_with("disposition audit phase");
    assert_eq!(audit_phase.max_items.get(), 1);
    assert_eq!(audit_phase.body.name().as_str(), "audit_disposition_stage");

    let judges = iteration_nodes
        .iter()
        .find_map(|node| match node {
            GraphNode::Map(node) if node.name.as_str() == "independent_judges" => Some(node),
            _ => None,
        })
        .assert_value_with("independent judges");
    assert_eq!(judges.max_items.get(), 3);
    assert_eq!(
        judges.over,
        DataSelector::State {
            path: field_path("judgeRoles").assert_value()
        }
    );
    assert_eq!(judges.body.name().as_str(), "research_judge");
    let judge = assert_role_verifier(
        iteration_nodes,
        "research_judge",
        "builtin.agent.research-judge@1",
        &["evidence", "method", "progress"],
    );
    assert!(judge.instructions.as_ref().is_some_and(|value| {
        value
            .as_str()
            .contains("restoring the known-invalid predecessor")
    }));
    assert_eq!(
        judge.signals.get(&field_name(VERDICT_FIELD).assert_value()),
        Some(&enum_labels(&["adopt", "record_only", "abort"]).assert_value())
    );
}

fn assert_role_verifier<'a>(
    nodes: &'a [&GraphNode],
    name: &str,
    worker: &str,
    roles: &[&str],
) -> &'a VerifierNode {
    let verifier = find_verifier(nodes, name);
    assert_eq!(verifier.worker.as_str(), worker);
    let PayloadType::Record { fields } = &verifier.input else {
        panic!("research role must have record input");
    };
    assert_required_role_field(fields, roles);
    assert!(
        !verifier
            .signals
            .contains_key(&field_name("role").assert_value())
    );
    verifier
}

fn find_verifier<'a>(nodes: &'a [&GraphNode], name: &str) -> &'a VerifierNode {
    nodes
        .iter()
        .find_map(|node| match node {
            GraphNode::Verifier(node) if node.name.as_str() == name => Some(node),
            _ => None,
        })
        .assert_value_with(name)
}

fn assert_recovery_route(iteration_nodes: &[&GraphNode], choice_name: &str, stage_name: &str) {
    let route = find_choice(iteration_nodes, choice_name);
    assert_eq!(
        route.branches.as_slice()[0].node.name().as_str(),
        stage_name
    );
}

fn assert_guarded_finalizers(iteration_nodes: &[&GraphNode]) {
    for name in ["abort_execution", "abort_scouting"] {
        let result_name = format!("{name}_result");
        let route = find_choice(iteration_nodes, &result_name);
        let Guard::In { value, labels } = &route.branches.as_slice()[0].when else {
            panic!("finalizer result must guard its worker error");
        };
        assert_eq!(value.name.as_str(), name);
        assert_eq!(value.source, ControlSource::Error);
        assert!(value.field.is_none());
        assert_eq!(labels, &worker_error_labels().assert_value());
        assert_eq!(
            route.branches.as_slice()[0].node.name().as_str(),
            format!("{name}_failed")
        );
        assert_eq!(
            route.otherwise.as_ref().assert_value().name().as_str(),
            format!("{name}_complete")
        );
        assert!(matches!(
            route.otherwise.as_ref().assert_value().as_ref(),
            GraphNode::Map(_)
        ));
    }
}

fn assert_research_checkpoint(
    iteration_nodes: &[&GraphNode],
    delivery: TemplateDelivery,
    expected_deliveries: usize,
) {
    assert_eq!(
        iteration_nodes
            .iter()
            .filter(|node| matches!(
                node,
                GraphNode::Verifier(node)
                    if node.worker.as_str() == GIT_DELIVERY_PUSH_WORKER_REF
            ))
            .count(),
        expected_deliveries
    );
    if delivery != TemplateDelivery::Push {
        assert!(
            iteration_nodes
                .iter()
                .all(|node| node.name().as_str() != "checkpoint_manifest")
        );
        return;
    }

    let manifest = iteration_nodes
        .iter()
        .find_map(|node| match node {
            GraphNode::Step(node) if node.name.as_str() == "checkpoint_manifest" => Some(node),
            _ => None,
        })
        .assert_value_with("research checkpoint manifest");
    assert!(
        manifest
            .instructions
            .as_ref()
            .is_some_and(|value| { value.as_str().contains("best supported historical finding") })
    );
    let manifest_result = find_choice(iteration_nodes, "checkpoint_manifest_result");
    assert_eq!(
        manifest_result.branches.as_slice()[0].node.name().as_str(),
        "checkpoint_manifest_failed"
    );

    let delivery_result = find_choice(iteration_nodes, "checkpoint_delivery_result");
    assert_eq!(
        delivery_result.branches.as_slice()[1].node.name().as_str(),
        "checkpoint_repair_required"
    );
    assert!(matches!(
        &delivery_result.branches.as_slice()[1].node,
        GraphNode::Fail(_)
    ));

    let auditor = find_verifier(iteration_nodes, "checkpoint_audit");
    assert_eq!(
        auditor
            .signals
            .get(&field_name(VERDICT_FIELD).assert_value()),
        Some(&enum_labels(&[ACCEPTED_LABEL, REJECTED_LABEL]).assert_value())
    );
    assert_eq!(auditor.diagnostic, diagnostic_type().assert_value());
    assert_eq!(auditor.write_bindings.len(), 1);

    let audit_result = find_choice(iteration_nodes, "checkpoint_audit_result");
    assert_eq!(
        audit_result.branches.as_slice()[1].node.name().as_str(),
        "checkpoint_audit_rejected"
    );
    assert!(matches!(
        &audit_result.branches.as_slice()[1].node,
        GraphNode::Fail(_)
    ));
    assert_eq!(
        audit_result.branches.as_slice()[2].node.name().as_str(),
        "checkpoint_accepted"
    );
}

fn find_choice<'a>(nodes: &'a [&GraphNode], name: &str) -> &'a ChoiceNode {
    nodes
        .iter()
        .find_map(|node| match node {
            GraphNode::Choice(node) if node.name.as_str() == name => Some(node),
            _ => None,
        })
        .assert_value_with(name)
}

fn assert_research_decision(iteration_nodes: &[&GraphNode]) {
    let decision = find_choice(iteration_nodes, "research_decision");
    assert_eq!(decision.branches.as_slice().len(), 5);
    assert_eq!(
        decision.branches.as_slice()[0].node.name().as_str(),
        "abort_review"
    );
    assert_eq!(
        decision.branches.as_slice()[1].node.name().as_str(),
        "judge_activation_overflow"
    );
    assert_disposition_routes(decision);
    assert_eq!(
        decision.otherwise.as_ref().assert_value().name().as_str(),
        "invalid_judge_consensus"
    );
    assert_disposition_stages(iteration_nodes);
}

fn assert_disposition_routes(decision: &ChoiceNode) {
    for (branch, count, label, target) in [
        (
            &decision.branches.as_slice()[2],
            1,
            "abort",
            "finalize_aborted",
        ),
        (
            &decision.branches.as_slice()[3],
            3,
            "adopt",
            "finalize_adopted",
        ),
        (
            &decision.branches.as_slice()[4],
            1,
            "record_only",
            "finalize_record_only",
        ),
    ] {
        let Guard::KOfMap {
            count: actual_count,
            value,
            labels,
        } = &branch.when
        else {
            panic!("research disposition must use a mapped verdict guard");
        };
        assert_eq!(actual_count.get(), count);
        assert_eq!(value.name.as_str(), "research_judge");
        assert_eq!(value.source, ControlSource::Signal);
        assert_eq!(
            value.field.as_ref().map(FieldName::as_str),
            Some(VERDICT_FIELD)
        );
        assert_eq!(labels, &enum_labels(&[label]).assert_value());
        assert_eq!(branch.node.name().as_str(), target);
    }
}

fn assert_disposition_stages(iteration_nodes: &[&GraphNode]) {
    for (finalizer_name, disposition) in [
        ("finalize_aborted", "abort"),
        ("finalize_adopted", "adopt"),
        ("finalize_record_only", "record_only"),
    ] {
        let finalizer = iteration_nodes
            .iter()
            .find_map(|node| match node {
                GraphNode::Step(node) if node.name.as_str() == finalizer_name => Some(node),
                _ => None,
            })
            .assert_value_with(finalizer_name);
        assert!(finalizer.instructions.as_ref().is_some_and(|value| {
            value
                .as_str()
                .contains(&format!("graph selected '{disposition}'"))
                && value.as_str().contains("independently auditable")
                && value
                    .as_str()
                    .contains("best supported historical findings")
        }));
    }

    let auditor = find_verifier(iteration_nodes, "audit_disposition");
    assert_eq!(
        auditor.worker.as_str(),
        "builtin.agent.research-disposition-auditor@1"
    );
    assert_eq!(
        auditor
            .signals
            .get(&field_name(VERDICT_FIELD).assert_value()),
        Some(&enum_labels(&[ACCEPTED_LABEL, REJECTED_LABEL]).assert_value())
    );
    assert!(auditor.instructions.as_ref().is_some_and(|value| {
        value.as_str().contains("required disposition")
            && value.as_str().contains("current filesystem")
    }));

    let audit_result = find_choice(iteration_nodes, "audit_disposition_result");
    assert_eq!(audit_result.branches.as_slice().len(), 3);
    assert!(matches!(
        audit_result.branches.as_slice()[0].node,
        GraphNode::Fail(_)
    ));
    assert!(matches!(
        audit_result.branches.as_slice()[1].node,
        GraphNode::Fail(_)
    ));
    assert_eq!(
        audit_result.branches.as_slice()[2].node.name().as_str(),
        "audit_disposition_accepted"
    );
}

#[test]
fn software_change_has_one_global_ten_cycle_budget() {
    for delivery in [
        TemplateDelivery::None,
        TemplateDelivery::Push,
        TemplateDelivery::PullRequest,
        TemplateDelivery::Merge,
    ] {
        let graph = BuiltinGraphTemplate::SoftwareChange
            .materialize(delivery)
            .assert_value();
        let loops = all_nodes(&graph.root)
            .into_iter()
            .filter_map(|node| match node {
                GraphNode::Loop(loop_node) => Some(loop_node),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(loops.len(), 1, "software-change must contain one loop");
        let change_loop = loops.first().assert_value_with("software-change loop");
        assert_eq!(change_loop.name.as_str(), "change_loop");
        assert_eq!(change_loop.max_iterations.get(), 10);
        assert!(change_loop.until.is_none());
    }
}

#[tokio::test]
async fn rejected_parallel_reviews_dispatch_repair_with_both_diagnostics() {
    let (verified, initial_input) = verified_software_template(TemplateDelivery::None).await;
    let history = [
        settled_worker(),
        settled_review(
            2,
            "acceptance",
            REJECTED_LABEL,
            "missing requested behavior",
        ),
        settled_review(3, "code", REJECTED_LABEL, "unsafe error handling"),
    ];
    let reduction = reduce(&verified, &initial_input, &history);

    assert!(reduction.decisions.iter().any(|decision| matches!(
        decision,
        Decision::Dispatch { occurrence, input, .. }
            if occurrence.node.as_str() == "review_repair"
                && input == &json!({
                    "task":"repair checkout",
                    "acceptanceFeedback":"missing requested behavior",
                    "codeFeedback":"unsafe error handling",
                    "deliveryFeedback":""
                })
    )));
}

fn crashed_parallel_review_history() -> Vec<DurableExecution> {
    let review_input = json!({"task":"repair checkout","deliveryFeedback":""});
    vec![
        settled_worker(),
        settled_review(
            2,
            "acceptance",
            REJECTED_LABEL,
            "missing requested behavior",
        ),
        settled_failure(
            SettledExecutionSpec {
                execution: 3,
                node_instance: 3,
                node: "code",
                settled_at: 3,
                input: review_input,
            },
            WorkerErrorCode::Crash,
        ),
    ]
}

#[tokio::test]
async fn crashed_parallel_review_retries_only_the_failed_verifier() {
    let (verified, initial_input) = verified_software_template(TemplateDelivery::None).await;
    let mut history = crashed_parallel_review_history();
    let reduction = reduce(&verified, &initial_input, &history);

    assert_eq!(
        reduction
            .decisions
            .iter()
            .filter(|decision| matches!(decision, Decision::Dispatch { .. }))
            .count(),
        1
    );
    assert!(reduction.decisions.iter().any(|decision| matches!(
        decision,
        Decision::Dispatch { occurrence, attempt, input, .. }
            if occurrence.node.as_str() == "code"
                && attempt.get() == MAX_AGENT_VERIFIER_ATTEMPTS
                && input == &json!({"task":"repair checkout","deliveryFeedback":""})
    )));
    assert!(reduction.terminal.is_none());

    let mut recovered = settled_review_execution(
        SettledExecutionSpec {
            execution: 4,
            node_instance: 3,
            node: "code",
            settled_at: 5,
            input: json!({"task":"repair checkout","deliveryFeedback":""}),
        },
        ACCEPTED_LABEL,
        "implementation sound",
    );
    recovered.attempt = PositiveInteger::new(MAX_AGENT_VERIFIER_ATTEMPTS).assert_value();
    history.push(recovered);
    let reviewed = reduce(&verified, &initial_input, &history);
    assert_dispatched_together(&reviewed, &["review_repair"]);
    let repair_input = json!({
        "task":"repair checkout",
        "acceptanceFeedback":"missing requested behavior",
        "codeFeedback":"implementation sound",
        "deliveryFeedback":""
    });
    assert!(reviewed.decisions.iter().any(|decision| matches!(
        decision, Decision::Dispatch { input, .. } if input == &repair_input
    )));
    history.push(settled_agent(SettledExecutionSpec {
        execution: 5,
        node_instance: 4,
        node: "review_repair",
        settled_at: 6,
        input: repair_input,
    }));
    let next_round = reduce(&verified, &initial_input, &history);
    assert_dispatched_together(&next_round, &["acceptance", "code"]);
    assert!(next_round.decisions.iter().all(|decision| match decision {
        Decision::Dispatch { attempt, .. } => attempt.get() == 1,
        _ => true,
    }));
}

#[tokio::test]
async fn repeatedly_crashed_parallel_review_fails_after_the_retry_limit() {
    let (verified, initial_input) = verified_software_template(TemplateDelivery::None).await;
    let review_input = json!({"task":"repair checkout","deliveryFeedback":""});
    let mut history = crashed_parallel_review_history();
    history.push(settled_failure_attempt(
        SettledExecutionSpec {
            execution: 4,
            node_instance: 3,
            node: "code",
            settled_at: 5,
            input: review_input,
        },
        WorkerErrorCode::Crash,
        MAX_AGENT_VERIFIER_ATTEMPTS,
    ));

    assert_eq!(
        reduce(&verified, &initial_input, &history).terminal,
        Some(TerminalProjection::Failed {
            reason: "review_failed".parse().assert_value()
        })
    );
}

#[tokio::test]
async fn accepted_second_review_round_ignores_stale_sibling_verdicts() {
    let (verified, initial_input) = verified_software_template(TemplateDelivery::None).await;
    let review_input = json!({"task":"repair checkout","deliveryFeedback":""});
    let mut history = vec![
        settled_worker(),
        settled_review(2, "acceptance", ACCEPTED_LABEL, "requirements met"),
        settled_review(3, "code", REJECTED_LABEL, "unsafe error handling"),
        settled_agent(SettledExecutionSpec {
            execution: 4,
            node_instance: 4,
            node: "review_repair",
            settled_at: 4,
            input: json!({
                "task":"repair checkout",
                "acceptanceFeedback":"requirements met",
                "codeFeedback":"unsafe error handling",
                "deliveryFeedback":""
            }),
        }),
    ];
    assert_dispatched_together(
        &reduce(&verified, &initial_input, &history),
        &["acceptance", "code"],
    );

    history.extend([
        settled_review_execution(
            SettledExecutionSpec {
                execution: 5,
                node_instance: 2,
                node: "acceptance",
                settled_at: 6,
                input: review_input.clone(),
            },
            ACCEPTED_LABEL,
            "repaired change meets requirements",
        ),
        settled_review_execution(
            SettledExecutionSpec {
                execution: 6,
                node_instance: 3,
                node: "code",
                settled_at: 5,
                input: review_input,
            },
            ACCEPTED_LABEL,
            "repaired implementation is sound",
        ),
    ]);

    assert_eq!(
        reduce(&verified, &initial_input, &history).terminal,
        Some(TerminalProjection::Succeeded {
            output: serde_json::Value::Null,
        })
    );
}

#[tokio::test]
async fn accepted_reviews_complete_or_dispatch_pull_request_delivery() {
    for delivery in [
        TemplateDelivery::None,
        TemplateDelivery::Push,
        TemplateDelivery::PullRequest,
    ] {
        let (verified, initial_input) = verified_software_template(delivery).await;
        let reviews = accepted_review_history(delivery);
        let worker_history = [settled_worker()];
        let after_worker = reduce(&verified, &initial_input, &worker_history);
        assert_dispatched_together(&after_worker, &["acceptance", "code"]);
        let reviewed = reduce(&verified, &initial_input, &reviews);

        if delivery == TemplateDelivery::None {
            assert_eq!(
                reviewed.terminal,
                Some(TerminalProjection::Succeeded {
                    output: serde_json::Value::Null,
                })
            );
            continue;
        }
        assert_dispatched_together(&reviewed, &[DELIVERY_NODE]);
        let (mode, outcome) = match delivery {
            TemplateDelivery::Push => (DeliveryMode::Push, DELIVERY_PUSHED_LABEL),
            TemplateDelivery::PullRequest => (DeliveryMode::PullRequestV2, DELIVERY_READY_LABEL),
            _ => unreachable!(),
        };
        let receipt = delivery_receipt(mode, outcome);
        let mut delivered = reviews;
        delivered.push(settled_delivery(
            SettledExecutionSpec {
                execution: 4,
                node_instance: 4,
                node: DELIVERY_NODE,
                settled_at: 4,
                input: delivery_input(),
            },
            mode,
            outcome,
        ));
        assert_eq!(
            reduce(&verified, &initial_input, &delivered).terminal,
            Some(TerminalProjection::Succeeded { output: receipt })
        );
    }
}

#[tokio::test]
async fn merge_delivery_repairs_recoverable_outcomes_then_returns_the_receipt() {
    for recoverable in [
        DELIVERY_CI_FAILED_LABEL,
        DELIVERY_CONFLICT_LABEL,
        DELIVERY_REPAIR_REQUIRED_LABEL,
    ] {
        assert_recoverable_delivery(recoverable).await;
    }
}

#[tokio::test]
async fn delivery_infrastructure_failure_never_dispatches_code_repair() {
    let (verified, initial_input) = verified_software_template(TemplateDelivery::Merge).await;
    let mut history = accepted_review_history(TemplateDelivery::Merge);
    history.push(settled_failure(
        SettledExecutionSpec {
            execution: 4,
            node_instance: 4,
            node: DELIVERY_NODE,
            settled_at: 4,
            input: delivery_input(),
        },
        WorkerErrorCode::Crash,
    ));

    let reduction = reduce(&verified, &initial_input, &history);
    assert_eq!(
        reduction.terminal,
        Some(TerminalProjection::Failed {
            reason: "delivery_failed".parse().assert_value()
        })
    );
    assert!(reduction.decisions.iter().all(|decision| !matches!(
        decision,
        Decision::Dispatch { occurrence, .. }
            if occurrence.node.as_str() == "delivery_repair"
    )));
}

async fn assert_recoverable_delivery(recoverable: &str) {
    let delivery_feedback = format!(
        "sourceRevision: {}\nreviewBaseRevision: {}\ntrusted delivery reported {recoverable}",
        "a".repeat(40),
        "b".repeat(40),
    );
    let (verified, initial_input) = verified_software_template(TemplateDelivery::Merge).await;
    let mut history = accepted_review_history(TemplateDelivery::Merge);
    let repair_input = json!({
        "task":"repair checkout",
        "outcome":recoverable,
        "deliveryFeedback":delivery_feedback
    });
    history.push(settled_delivery_with_diagnostic(
        SettledExecutionSpec {
            execution: 4,
            node_instance: 4,
            node: DELIVERY_NODE,
            settled_at: 4,
            input: delivery_input(),
        },
        DeliveryMode::MergeV3,
        recoverable,
        &delivery_feedback,
    ));
    assert_dispatch(
        &reduce(&verified, &initial_input, &history),
        "delivery_repair",
        &repair_input,
    );
    history.push(settled_agent(SettledExecutionSpec {
        execution: 5,
        node_instance: 5,
        node: "delivery_repair",
        settled_at: 5,
        input: repair_input,
    }));
    assert_review_repair_preserves_delivery_feedback(
        &verified,
        &initial_input,
        &mut history,
        &delivery_feedback,
    );
    assert_repaired_reviews_then_merge(&verified, &initial_input, &mut history, &delivery_feedback);
}

fn assert_review_repair_preserves_delivery_feedback(
    verified: &VerifiedGraph,
    initial_input: &Value,
    history: &mut Vec<DurableExecution>,
    delivery_feedback: &str,
) {
    let review_input = json!({"task":"repair checkout","deliveryFeedback":delivery_feedback});
    let reviews = reduce(verified, initial_input, history);
    assert_dispatch(&reviews, "acceptance", &review_input);
    assert_dispatch(&reviews, "code", &review_input);
    history.extend([
        settled_review_execution_with_output(
            SettledExecutionSpec {
                execution: 6,
                node_instance: 2,
                node: "acceptance",
                settled_at: 6,
                input: review_input.clone(),
            },
            ACCEPTED_LABEL,
            "requirements met after integration",
            repaired_change_manifest(),
        ),
        settled_review_execution(
            SettledExecutionSpec {
                execution: 7,
                node_instance: 3,
                node: "code",
                settled_at: 7,
                input: review_input,
            },
            REJECTED_LABEL,
            "repair an integration defect",
        ),
    ]);
    let repair_input = json!({
        "task":"repair checkout",
        "acceptanceFeedback":"requirements met after integration",
        "codeFeedback":"repair an integration defect",
        "deliveryFeedback":delivery_feedback,
    });
    assert_dispatch(
        &reduce(verified, initial_input, history),
        "review_repair",
        &repair_input,
    );
    history.push(settled_agent(SettledExecutionSpec {
        execution: 8,
        node_instance: 6,
        node: "review_repair",
        settled_at: 8,
        input: repair_input,
    }));
}

fn assert_repaired_reviews_then_merge(
    verified: &VerifiedGraph,
    initial_input: &Value,
    history: &mut Vec<DurableExecution>,
    delivery_feedback: &str,
) {
    let repaired = reduce(verified, initial_input, history);
    assert_dispatched_together(&repaired, &["acceptance", "code"]);
    let review_input = json!({
        "task":"repair checkout",
        "deliveryFeedback":delivery_feedback
    });
    assert_dispatch(&repaired, "acceptance", &review_input);
    assert_dispatch(&repaired, "code", &review_input);
    history.push(settled_review_execution_with_output(
        SettledExecutionSpec {
            execution: 9,
            node_instance: 2,
            node: "acceptance",
            settled_at: 9,
            input: review_input.clone(),
        },
        ACCEPTED_LABEL,
        "CI repair meets the request",
        repaired_change_manifest(),
    ));
    history.push(settled_review_execution(
        SettledExecutionSpec {
            execution: 10,
            node_instance: 3,
            node: "code",
            settled_at: 10,
            input: review_input,
        },
        ACCEPTED_LABEL,
        "CI repair is sound",
    ));
    assert_dispatch(
        &reduce(verified, initial_input, history),
        DELIVERY_NODE,
        &repaired_delivery_input(),
    );
    history.push(settled_delivery(
        SettledExecutionSpec {
            execution: 11,
            node_instance: 4,
            node: DELIVERY_NODE,
            settled_at: 11,
            input: repaired_delivery_input(),
        },
        DeliveryMode::MergeV3,
        DELIVERY_MERGED_LABEL,
    ));
    let receipt = delivery_receipt(DeliveryMode::MergeV3, DELIVERY_MERGED_LABEL);
    assert_eq!(
        reduce(verified, initial_input, history).terminal,
        Some(TerminalProjection::Succeeded { output: receipt })
    );
}

async fn assert_admissible(
    template: BuiltinGraphTemplate,
    delivery: TemplateDelivery,
    expected: &[&str],
) {
    let graph = template.materialize(delivery).assert_value();
    let leaves = executable_leaves(&graph.root);
    let names = leaves
        .iter()
        .map(|node| node.name().as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(names, expected.iter().copied().collect());
    assert_instruction_ownership(&leaves);

    let runtime = runtime_for(template, delivery, &leaves);
    let initial_input = json!({"task":"implement the requested change"});
    let policy = if delivery == TemplateDelivery::None {
        DeliveryPolicy::Optional
    } else {
        DeliveryPolicy::Required
    };
    let admitted = NativeV2Admission
        .validate_intent(
            &RunSubmissionIntent {
                title: RunTitle::new("Built-in template admission").assert_value(),
                graph,
                initial_input,
                runtime,
                branch: None,
                submission_key: IdempotencyKey::new(format!(
                    "template-{}-{}",
                    template.name(),
                    delivery.name()
                ))
                .assert_value(),
            },
            policy,
        )
        .await;
    admitted.unwrap_or_else(|error| {
        panic!(
            "{} with {} delivery was not admissible: {error:?}",
            template.name(),
            delivery.name()
        )
    });
}

async fn verified_software_template(
    delivery: TemplateDelivery,
) -> (
    openengine_cluster_server::admission::VerifiedGraph,
    serde_json::Value,
) {
    let template = BuiltinGraphTemplate::SoftwareChange;
    let graph = template.materialize(delivery).assert_value();
    let runtime = runtime_for(template, delivery, &executable_leaves(&graph.root));
    let initial_input = if delivery == TemplateDelivery::None {
        json!({"task":"repair checkout"})
    } else {
        json!({"task":"repair checkout","issueNumber":"208"})
    };
    let admitted = NativeV2Admission
        .admit(RunSubmission {
            title: RunTitle::new("Template behavior").assert_value(),
            graph,
            initial_input: initial_input.clone(),
            runtime,
            source: resolved_source(),
            submission_key: IdempotencyKey::new(format!("template-behavior-{}", delivery.name()))
                .assert_value(),
        })
        .await
        .assert_value();
    (
        openengine_cluster_server::admission::VerifiedGraph {
            compiled_ir: admitted.graph,
            diagnostics: Vec::new(),
        },
        initial_input,
    )
}

fn reduce(
    verified: &openengine_cluster_server::admission::VerifiedGraph,
    initial_input: &serde_json::Value,
    history: &[DurableExecution],
) -> Reduction {
    let next_node_instance = history
        .iter()
        .map(|execution| execution.node_instance.get())
        .max()
        .unwrap_or(0)
        + 1;
    let next_execution = history
        .iter()
        .map(|execution| execution.execution.get())
        .max()
        .unwrap_or(0)
        + 1;
    FullV1Reducer::native_v2(verified)
        .reduce(ReductionInput {
            initial_input,
            executions: history,
            next_node_instance,
            next_execution,
        })
        .assert_value()
}

fn assert_dispatched_together(reduction: &Reduction, expected: &[&str]) {
    let dispatched = reduction
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Dispatch { occurrence, .. } => Some(occurrence.node.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(dispatched, expected.iter().copied().collect());
}

fn assert_dispatch(reduction: &Reduction, node: &str, expected_input: &serde_json::Value) {
    assert!(
        reduction.decisions.iter().any(|decision| matches!(
            decision,
            Decision::Dispatch { occurrence, input, .. }
                if occurrence.node.as_str() == node && input == expected_input
        )),
        "expected {node} input {expected_input}; decisions: {:?}",
        reduction.decisions
    );
}

fn all_nodes(root: &GraphNode) -> Vec<&GraphNode> {
    let mut nodes = Vec::new();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        nodes.push(node);
        pending.extend(openengine_cluster_server::graph_verifier::graph_node_children(node));
    }
    nodes
}

#[tokio::test]
async fn pull_request_delivery_routes_a_pre_review_git_failure_to_repair() {
    let (verified, initial_input) = verified_software_template(TemplateDelivery::PullRequest).await;
    let mut history = accepted_review_history(TemplateDelivery::PullRequest);
    history.push(settled_delivery_with_diagnostic(
        SettledExecutionSpec {
            execution: 4,
            node_instance: 4,
            node: DELIVERY_NODE,
            settled_at: 4,
            input: delivery_input(),
        },
        DeliveryMode::PullRequestV2,
        DELIVERY_REPAIR_REQUIRED_LABEL,
        "git push: unfamiliar failure",
    ));
    assert_dispatch(
        &reduce(&verified, &initial_input, &history),
        "delivery_repair",
        &json!({
            "task":"repair checkout", "outcome":"repair_required",
            "deliveryFeedback":"git push: unfamiliar failure"
        }),
    );
}
