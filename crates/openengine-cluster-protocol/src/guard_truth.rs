//! Three-valued control evaluation for joins. Missing racer facts stay unknown.
use crate::{ControlSelector, EnumLabel, Guard};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardTruth {
    True,
    False,
    Unknown,
}

fn threshold(count: u64, values: impl ExactSizeIterator<Item = GuardTruth>) -> GuardTruth {
    let mut matching = 0;
    let mut unknown = 0;
    let mut remaining = values.len() as u64;
    for value in values {
        remaining -= 1;
        match value {
            GuardTruth::True => matching += 1,
            GuardTruth::Unknown => unknown += 1,
            GuardTruth::False => {}
        }
        if matching >= count {
            return GuardTruth::True;
        }
        if matching + unknown + remaining < count {
            return GuardTruth::False;
        }
    }
    GuardTruth::Unknown
}

/// Matching counts exist only for available controls; unknown facts never become false.
pub fn evaluate_partial_guard(
    guard: &Guard,
    select: &impl Fn(&ControlSelector, &[EnumLabel], bool) -> Option<u64>,
) -> GuardTruth {
    let compare =
        |value, labels: &[EnumLabel], aggregate, count| match select(value, labels, aggregate) {
            Some(actual) if actual >= count => GuardTruth::True,
            Some(_) => GuardTruth::False,
            None => GuardTruth::Unknown,
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
                .map(|guard| evaluate_partial_guard(guard, select)),
        ),
        Guard::Any { guards } => threshold(
            1,
            guards
                .as_slice()
                .iter()
                .map(|guard| evaluate_partial_guard(guard, select)),
        ),
        Guard::Not { guard } => match evaluate_partial_guard(guard, select) {
            GuardTruth::True => GuardTruth::False,
            GuardTruth::False => GuardTruth::True,
            GuardTruth::Unknown => GuardTruth::Unknown,
        },
    }
}

#[cfg(test)]
#[path = "guard_truth/tests.rs"]
mod tests;
