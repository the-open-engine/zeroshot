use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::*;
use crate::native_v2_admission::executable_runtime_roles;
use crate::native_v2_cli::{
    BuiltinGraphTemplate, NativeV2CliCommand, TemplateDelivery,
    try_execute_native_v2_static_with_terminal,
};

fn node(value: Value) -> GraphNode {
    serde_json::from_value(value).assert_value()
}

fn step(name: &str) -> Value {
    json!({"kind":"step", "name":name, "worker":"test.worker@1", "input":{"kind":"null"},
        "output":{"kind":"null"}, "inputBindings":[], "writeBindings":[], "attempts":1})
}

fn done(name: &str) -> Value {
    json!({"kind":"succeed", "name":name, "output":{"kind":"null"}, "bindings":[]})
}

fn guard() -> Guard {
    serde_json::from_value(
        json!({"kind":"in", "value":{"source":"signal", "name":"review",
        "field":"verdict"}, "labels":["accepted"]}),
    )
    .assert_value()
}

fn render(root: GraphNode) -> String {
    let mut graph = BuiltinGraphTemplate::SingleWorker
        .materialize(TemplateDelivery::None)
        .assert_value();
    graph.root = root;
    let mut output = Vec::new();
    write_graph_tree(&graph, &mut output).assert_value();
    String::from_utf8(output).assert_value()
}

#[test]
fn every_node_kind_preserves_structure_names_bounds_signals_and_branch_guards() {
    let root = node(
        json!({"kind":"seq", "name":"root", "state":{"kind":"null"}, "promotedStatePaths":[],
        "children":[
            step("worker"),
            {"kind":"par", "name":"reviews", "state":{"kind":"null"}, "promotedStatePaths":[],
             "join":{"kind":"all"}, "branches":[
                {"kind":"verifier", "name":"review", "worker":"test.review@1", "input":{"kind":"null"},
                 "output":{"kind":"null"}, "inputBindings":[], "writeBindings":[], "attempts":2,
                 "signals":{"verdict":["accepted","rejected"], "risk":["low","high"]}, "diagnostic":{"kind":"null"}},
                {"kind":"loop", "name":"retry", "state":{"kind":"null"}, "promotedStatePaths":[],
                 "maxIterations":10, "maxIterationsInput":["options","iterations"], "until":guard(),
                 "body":{"kind":"map", "name":"batch", "state":{"kind":"null"}, "promotedStatePaths":[],
                     "maxItems":3, "over":{"source":"state","path":["tasks"]}, "body":step("delivery_repair")}}
             ]},
            {"kind":"choice", "name":"result", "state":{"kind":"null"}, "promotedStatePaths":[],
             "branches":[{"when":guard(), "node":done("finished")}],
             "otherwise":{"kind":"fail", "name":"failed", "reason":"review_failed"}}
        ]}),
    );
    assert_eq!(
        render(root),
        concat!(
            "root  seq\n",
            "├─ worker *  step  1 attempt\n",
            "├─ reviews  par  wait for all\n",
            "│  ├─ review *  verifier  2 attempts  risk: high | low; verdict: accepted | rejected\n",
            "│  └─ retry  loop  max 10 iterations (input: options.iterations); until review.signal.verdict in [accepted]\n",
            "│     └─ batch  map  over state.tasks; max 3 items\n",
            "│        └─ delivery_repair *  step  1 attempt\n",
            "└─ result  choice  first matching branch\n",
            "   ├─ review.signal.verdict in [accepted] → finished  succeed\n",
            "   └─ otherwise → failed  fail  reason: review_failed\n",
            "\n* executable node: RuntimePlan binding (template delivery bindings are automatic)\n"
        )
    );
}

#[test]
fn guard_labels_keep_boolean_grouping_thresholds_and_control_sources() {
    let atom = guard();
    let error: Guard = serde_json::from_value(
        json!({"kind":"in", "value":{"source":"error", "name":"worker"},
        "labels":["timeout", "crash"]}),
    )
    .assert_value();
    let group = ControlSelector {
        source: ControlSource::Group,
        name: openengine_cluster_protocol::NodeName::new("batch").assert_value(),
        field: None,
    };
    let cases = [
        (
            json!({"kind":"all", "guards":[atom, {"kind":"any", "guards":[atom, error]}]}),
            "(review.signal.verdict in [accepted] and (review.signal.verdict in [accepted] or worker.error in [crash | timeout]))",
        ),
        (
            json!({"kind":"not", "guard":atom}),
            "not (review.signal.verdict in [accepted])",
        ),
        (
            json!({"kind":"k_of_n", "count":2, "values":[group, {"source":"signal","name":"review","field":"verdict"}],
            "labels":["accepted"]}),
            "at least 2 of (batch.group, review.signal.verdict) in [accepted]",
        ),
        (
            json!({"kind":"k_of_map", "count":1, "value":group, "labels":["done"]}),
            "at least 1 mapped batch.group in [done]",
        ),
    ];
    for (value, expected) in cases {
        let guard: Guard = serde_json::from_value(value).assert_value();
        assert_eq!(guard_label(&guard), expected);
        let choice = node(
            json!({"kind":"choice", "name":"decision", "state":{"kind":"null"},
            "promotedStatePaths":[], "branches":[{"when":guard, "node":done("yes")}], "otherwise":null}),
        );
        assert!(render(choice).contains(&format!("└─ {expected} → yes  succeed\n")));
    }
}

#[test]
fn parallel_join_modes_and_item_selectors_are_readable() {
    for (join, expected) in [
        (Join::All {}, "wait for all".to_owned()),
        (Join::Any {}, "wait for any".to_owned()),
        (
            Join::Quorum {
                count: PositiveInteger::new(2).assert_value(),
            },
            "wait for 2".to_owned(),
        ),
        (
            Join::First { when: guard() },
            "first matching review.signal.verdict in [accepted]".to_owned(),
        ),
    ] {
        let par = node(
            json!({"kind":"par", "name":"parallel", "state":{"kind":"null"},
            "promotedStatePaths":[], "join":join, "branches":[step("one"),step("two")]}),
        );
        assert!(render(par).starts_with(&format!("parallel  par  {expected}\n")));
    }
    let mapped = node(
        json!({"kind":"map", "name":"nested", "state":{"kind":"null"},
        "promotedStatePaths":[], "over":{"source":"item","path":["nested","tasks"]},
        "maxItems":1, "body":step("writer")}),
    );
    assert!(render(mapped).contains("map  over item.nested.tasks; max 1 item\n"));
}

#[test]
fn all_supported_template_deliveries_mark_exactly_the_runtime_binding_names_and_preserve_json() {
    for template in BuiltinGraphTemplate::all() {
        for delivery in [
            TemplateDelivery::None,
            TemplateDelivery::Push,
            TemplateDelivery::PullRequest,
            TemplateDelivery::Merge,
            TemplateDelivery::Comment,
        ] {
            let Ok(graph) = template.materialize(delivery) else {
                continue;
            };
            let command = NativeV2CliCommand::TemplateShow {
                template: *template,
                delivery,
                json: false,
            };
            let mut output = Vec::new();
            try_execute_native_v2_static_with_terminal(&command, &mut output, true)
                .assert_value()
                .assert_value();
            let text = String::from_utf8(output).assert_value();
            let roles = executable_runtime_roles(&graph.root);
            assert_eq!(
                text.matches(" *  ").count(),
                roles.len(),
                "{template:?}/{delivery:?}"
            );
            for (name, _) in roles {
                assert!(
                    text.contains(&format!("{name} *  ")),
                    "{template:?}/{delivery:?}: {name}"
                );
            }
            for (terminal, json) in [(false, false), (false, true), (true, true)] {
                let command = NativeV2CliCommand::TemplateShow {
                    template: *template,
                    delivery,
                    json,
                };
                let mut output = Vec::new();
                try_execute_native_v2_static_with_terminal(&command, &mut output, terminal)
                    .assert_value()
                    .assert_value();
                assert_eq!(
                    serde_json::from_slice::<GraphSpec>(&output).assert_value(),
                    graph
                );
                let mut expected = serde_json::to_vec(&graph).assert_value();
                expected.push(b'\n');
                assert_eq!(output, expected);
            }
        }
    }
}

#[test]
fn tree_output_errors_are_propagated() {
    let graph = BuiltinGraphTemplate::SingleWorker
        .materialize(TemplateDelivery::None)
        .assert_value();
    assert!(write_graph_tree(&graph, &mut io::Cursor::new(&mut [0_u8; 1][..])).is_err());
}
