//! Three-valued control evaluation for joins. Missing racer facts stay unknown.
use crate::{ControlSelector, EnumLabel, Guard};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardTruth {
    True,
    False,
    Unknown,
}
const TRUE: u8 = 1;
const FALSE: u8 = 2;
const UNKNOWN: u8 = 4;

fn threshold(count: u64, values: impl IntoIterator<Item = u8>) -> u8 {
    let mut forced_true = 0;
    let mut possible_true = 0;
    let mut minimum_non_false = 0;
    let mut maximum_non_false = 0;
    let mut unknown = false;
    for value in values {
        forced_true += u64::from(value == TRUE);
        possible_true += u64::from(value & TRUE != 0);
        minimum_non_false += u64::from(value & FALSE == 0);
        maximum_non_false += u64::from(value & (TRUE | UNKNOWN) != 0);
        unknown |= value & UNKNOWN != 0;
    }
    let mut result = 0;
    if possible_true >= count {
        result |= TRUE;
    }
    if minimum_non_false < count {
        result |= FALSE;
    }
    if unknown && forced_true < count && maximum_non_false >= count {
        result |= UNKNOWN;
    }
    result
}

fn possibilities(
    guard: &Guard,
    select: &impl Fn(&ControlSelector, &[EnumLabel], bool) -> Option<u64>,
) -> u8 {
    let compare =
        |value, labels: &[EnumLabel], aggregate, count| match select(value, labels, aggregate) {
            Some(actual) if actual >= count => TRUE,
            Some(_) => FALSE,
            None => UNKNOWN,
        };
    match guard {
        Guard::In { value, labels } => compare(value, labels.values(), false, 1),
        Guard::KOfMap {
            value,
            labels,
            count,
        } => compare(value, labels.values(), true, count.get()),
        Guard::KOfN {
            values,
            labels,
            count,
        } => threshold(
            count.get(),
            values
                .as_slice()
                .iter()
                .map(|value| compare(value, labels.values(), false, 1)),
        ),
        Guard::All { guards } => threshold(
            guards.as_slice().len() as u64,
            guards
                .as_slice()
                .iter()
                .map(|guard| possibilities(guard, select)),
        ),
        Guard::Any { guards } => threshold(
            1,
            guards
                .as_slice()
                .iter()
                .map(|guard| possibilities(guard, select)),
        ),
        Guard::Not { guard } => {
            let value = possibilities(guard, select);
            ((value & TRUE) << 1) | ((value & FALSE) >> 1) | (value & UNKNOWN)
        }
    }
}

/// Matching counts exist only for available controls; unknown facts never become false.
pub fn evaluate_partial_guard(
    guard: &Guard,
    select: &impl Fn(&ControlSelector, &[EnumLabel], bool) -> Option<u64>,
) -> GuardTruth {
    match possibilities(guard, select) {
        TRUE => GuardTruth::True,
        FALSE => GuardTruth::False,
        _ => GuardTruth::Unknown,
    }
}

#[cfg(test)]
mod tests {
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
}
