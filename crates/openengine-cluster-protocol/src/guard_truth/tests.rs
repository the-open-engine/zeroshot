use super::*;
use serde_json::{Value, json};

fn selector(name: &str) -> Value {
    json!({"name":name,"source":"error","field":null})
}

fn atom(name: &str) -> Value {
    json!({"kind":"in","value":selector(name),"labels":["crash"]})
}

fn evaluate(value: Value) -> GuardTruth {
    let guard: Guard = serde_json::from_value(value).unwrap();
    evaluate_partial_guard(&guard, &|selector, _, _| match selector.name.as_str() {
        "yes" => Some(1),
        "no" => Some(0),
        _ => None,
    })
}

#[test]
fn all_and_any_follow_three_valued_truth_tables() {
    use GuardTruth::{False, True, Unknown};
    for (left, right, all, any) in [
        ("yes", "yes", True, True),
        ("yes", "no", False, True),
        ("yes", "unknown", Unknown, True),
        ("no", "yes", False, True),
        ("no", "no", False, False),
        ("no", "unknown", False, Unknown),
        ("unknown", "yes", Unknown, True),
        ("unknown", "no", False, Unknown),
        ("unknown", "unknown", Unknown, Unknown),
    ] {
        for (kind, expected) in [("all", all), ("any", any)] {
            assert_eq!(
                evaluate(json!({"kind":kind,"guards":[atom(left),atom(right)]})),
                expected,
                "{kind}({left}, {right})"
            );
        }
    }
}

#[test]
fn negation_preserves_unknown_in_nested_guards() {
    for (name, expected) in [
        ("yes", GuardTruth::False),
        ("no", GuardTruth::True),
        ("unknown", GuardTruth::Unknown),
    ] {
        assert_eq!(evaluate(json!({"kind":"not","guard":atom(name)})), expected);
    }
    assert_eq!(
        evaluate(
            json!({"kind":"not","guard":{"kind":"any","guards":[atom("no"),atom("unknown")]}})
        ),
        GuardTruth::Unknown
    );
}

#[test]
fn k_of_n_uses_available_matches_and_keeps_undecided_thresholds_unknown() {
    for (count, expected) in [
        (1, GuardTruth::True),
        (2, GuardTruth::Unknown),
        (3, GuardTruth::False),
    ] {
        assert_eq!(
            evaluate(json!({"kind":"k_of_n","count":count,
                "values":[selector("yes"),selector("unknown"),selector("no")],"labels":["crash"]})),
            expected
        );
    }
    assert_eq!(
        evaluate(json!({"kind":"k_of_n","count":2,
            "values":[selector("unknown1"),selector("unknown2")],"labels":["crash"]})),
        GuardTruth::Unknown
    );
}

#[test]
fn k_of_map_preserves_authored_count_and_aggregate_availability() {
    for (actual, count, expected) in [
        (Some(2), 2, GuardTruth::True),
        (Some(2), 3, GuardTruth::False),
        (None, 3, GuardTruth::Unknown),
    ] {
        let guard: Guard = serde_json::from_value(json!({"kind":"k_of_map","count":count,
            "value":selector("mapped"),"labels":["crash"]}))
        .unwrap();
        assert_eq!(
            evaluate_partial_guard(&guard, &|_, _, aggregate| {
                assert!(aggregate);
                actual
            }),
            expected
        );
    }
}

fn assert_short_circuit(value: Value, expected: GuardTruth, selected: &[&str]) {
    let guard: Guard = serde_json::from_value(value).unwrap();
    let calls = std::cell::RefCell::new(Vec::new());
    assert_eq!(
        evaluate_partial_guard(&guard, &|selector, _, _| {
            calls.borrow_mut().push(selector.name.as_str().to_owned());
            match selector.name.as_str() {
                "yes" => Some(1),
                "no" => Some(0),
                "unknown" => None,
                name => panic!("short-circuit must skip {name}"),
            }
        }),
        expected
    );
    assert_eq!(*calls.borrow(), selected);
}

#[test]
fn boolean_guards_stop_when_decided_and_continue_past_unknown() {
    for (kind, first, expected) in [
        ("all", "no", GuardTruth::False),
        ("any", "yes", GuardTruth::True),
    ] {
        assert_short_circuit(
            json!({"kind":kind,"guards":[atom(first),atom("unused")]}),
            expected,
            &[first],
        );
        assert_short_circuit(
            json!({"kind":kind,"guards":[atom("unknown"),atom(first),atom("unused")]}),
            expected,
            &["unknown", first],
        );
    }
}

#[test]
fn k_of_n_stops_when_met_or_impossible_without_guessing_unknowns() {
    for (count, names, expected, calls) in [
        (1, vec!["yes", "unused"], GuardTruth::True, vec!["yes"]),
        (2, vec!["no", "unused"], GuardTruth::False, vec!["no"]),
        (
            2,
            vec!["unknown", "yes"],
            GuardTruth::Unknown,
            vec!["unknown", "yes"],
        ),
        (
            2,
            vec!["unknown", "no", "no"],
            GuardTruth::False,
            vec!["unknown", "no", "no"],
        ),
    ] {
        let values = names.iter().map(|name| selector(name)).collect::<Vec<_>>();
        assert_short_circuit(
            json!({"kind":"k_of_n","count":count,"values":values,"labels":["crash"]}),
            expected,
            &calls,
        );
    }
}
