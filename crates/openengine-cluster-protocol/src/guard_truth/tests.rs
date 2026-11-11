use super::*;
use serde_json::json;

#[test]
fn three_valued_thresholds_and_negation_preserve_unknown() {
    let atom = |name: &str| json!({"kind":"in","value":{"name":name,"source":"error","field":null},"labels":["crash"]});
    let evaluate = |value| {
        let guard: Guard = serde_json::from_value(value).unwrap();
        evaluate_partial_guard(&guard, &|selector, _, _| match selector.name.as_str() {
            "yes" => Some(1),
            "no" => Some(0),
            _ => None,
        })
    };
    for (kind, left, right, expected) in [
        ("all", "yes", "unknown", GuardTruth::Unknown),
        ("all", "no", "unknown", GuardTruth::False),
        ("any", "yes", "unknown", GuardTruth::True),
        ("any", "no", "unknown", GuardTruth::Unknown),
    ] {
        assert_eq!(
            evaluate(json!({"kind":kind,"guards":[atom(left),atom(right)]})),
            expected
        );
    }
    assert_eq!(
        evaluate(json!({"kind":"not","guard":atom("unknown")})),
        GuardTruth::Unknown
    );
    for (count, expected) in [
        (1, GuardTruth::True),
        (2, GuardTruth::Unknown),
        (3, GuardTruth::False),
    ] {
        assert_eq!(
            evaluate(
                json!({"kind":"k_of_n","count":count,"values":[{"name":"yes","source":"error","field":null},{"name":"unknown","source":"error","field":null},{"name":"no","source":"error","field":null}],"labels":["crash"]})
            ),
            expected
        );
    }
    assert_eq!(
        evaluate(
            json!({"kind":"k_of_map","count":1,"value":{"name":"unknown","source":"error","field":null},"labels":["crash"]})
        ),
        GuardTruth::Unknown
    );
}
