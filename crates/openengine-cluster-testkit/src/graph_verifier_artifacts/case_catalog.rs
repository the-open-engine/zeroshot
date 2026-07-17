pub(super) fn positive_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("positive/basic.json", basic_graph()),
        ("positive/binding-channels.json", binding_channels_graph()),
        ("positive/guard-in.json", guarded_graph(in_guard())),
        (
            "positive/guard-all.json",
            guarded_graph(json!({"kind":"all","guards":[in_guard()]})),
        ),
        (
            "positive/guard-any.json",
            guarded_graph(json!({"kind":"any","guards":[in_guard(), error_guard()]})),
        ),
        (
            "positive/guard-not.json",
            guarded_graph(json!({"kind":"not","guard":in_guard()})),
        ),
        ("positive/guard-k-of-n.json", guarded_graph(k_of_n_guard())),
        ("positive/map-item-k-of-map.json", map_item_graph()),
        ("positive/map-signal-and-group.json", map_signal_graph()),
        ("positive/loop-and-group.json", loop_graph()),
        ("positive/join-all.json", parallel_graph("all")),
        ("positive/join-any.json", parallel_graph("any")),
        ("positive/join-quorum.json", parallel_graph("quorum")),
        ("positive/join-first.json", parallel_graph("first")),
        ("positive/nested-structural-folds.json", nested_fold_graph()),
        (
            "positive/exhaustive-terminal-choice.json",
            exhaustive_terminal_choice_graph(),
        ),
        (
            "positive/success-routed-write.json",
            success_routed_write_graph(),
        ),
        (
            "positive/map-indexed-promotion.json",
            map_indexed_promotion_graph(),
        ),
        (
            "positive/parallel-success-routed-promotion.json",
            parallel_success_routed_promotion_graph(),
        ),
    ]
}

pub(super) fn negative_cases() -> Vec<(&'static str, Value)> {
    let mut cases = vec![
        ("negative/duplicate-node.json", duplicate_node_graph()),
        ("negative/terminal-fallthrough.json", fallthrough_graph()),
        (
            "negative/illegal-control-selector.json",
            illegal_control_graph(),
        ),
        ("negative/undefined-read.json", undefined_read_graph()),
        (
            "negative/output-write-error-path.json",
            output_write_error_path_graph(),
        ),
        ("negative/cyclic-read.json", cyclic_read_graph()),
        ("negative/type-mismatch.json", type_mismatch_graph()),
        ("negative/dead-choice.json", dead_choice_graph()),
        ("negative/dead-otherwise.json", dead_otherwise_graph()),
        (
            "negative/non-exhaustive-choice.json",
            non_exhaustive_choice_graph(),
        ),
        (
            "negative/unsatisfiable-loop.json",
            unsatisfiable_loop_graph(),
        ),
        ("negative/invalid-quorum.json", invalid_quorum_graph()),
        (
            "negative/parallel-write-conflict.json",
            write_conflict_graph(),
        ),
        ("negative/unsafe-promotion.json", unsafe_promotion_graph()),
        (
            "negative/impossible-map-outcomes.json",
            impossible_map_outcomes_graph(),
        ),
        ("negative/closed-k-labels.json", closed_k_labels_graph()),
        (
            "negative/map-indexed-promotion-element-type.json",
            map_indexed_promotion_element_type_graph(),
        ),
        (
            "negative/map-promotion-target-not-array.json",
            map_promotion_target_not_array_graph(),
        ),
        (
            "negative/map-promotion-no-body-write.json",
            map_promotion_no_body_write_graph(),
        ),
        (
            "negative/parallel-failure-promotion.json",
            parallel_failure_promotion_graph(),
        ),
        (
            "negative/unconstructible-worker-input.json",
            unconstructible_worker_input_graph(),
        ),
        (
            "negative/unconstructible-terminal-output.json",
            unconstructible_terminal_output_graph(),
        ),
    ];
    cases.extend(registry_negative_cases());
    cases
}

pub(super) fn registry_negative_cases() -> Vec<(&'static str, Value)> {
    vec![
        (
            "negative/registry-not-found.json",
            worker_graph("fixture.missing@1"),
        ),
        (
            "negative/registry-version-unavailable.json",
            worker_graph("fixture.version-unavailable@2"),
        ),
        (
            "negative/registry-descriptor-contract.json",
            worker_graph("fixture.invalid-contract@1"),
        ),
        (
            "negative/registry-descriptor-identity.json",
            worker_graph("fixture.identity@1"),
        ),
        (
            "negative/registry-graph-profile.json",
            worker_graph("fixture.profile@1"),
        ),
        ("negative/registry-input.json", registry_input_graph()),
        ("negative/registry-output.json", registry_output_graph()),
        (
            "negative/registry-verifier-contract.json",
            registry_verifier_contract_graph(),
        ),
        (
            "negative/registry-signal-field.json",
            registry_signal_field_graph(),
        ),
        (
            "negative/registry-signal-labels.json",
            registry_signal_labels_graph(),
        ),
        (
            "negative/registry-diagnostic.json",
            registry_diagnostic_graph(),
        ),
    ]
}
use super::fixture_builders::*;
use super::fixture_controls::*;
use super::fixture_negative::*;
use super::*;
