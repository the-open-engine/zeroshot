//! Authored guards and completion predicates as conditions over control variables.
use std::rc::Rc;

use super::*;

/// A node's status variable and which of its values let the node emit controls.
pub(super) type Gate = (u32, Rc<[bool]>);

/// Where one selector's control comes from. A gated control is empty unless its node's
/// status variable takes an emitting value.
#[derive(Clone)]
pub(in crate::graph_verifier) struct Reader {
    pub(super) gate: Option<Gate>,
    pub(super) variable: u32,
    pub(super) controls: Rc<[AssignedControl]>,
}

pub(in crate::graph_verifier) type Controls = BTreeMap<SelectorKey, Reader>;

pub(super) fn empty_control() -> AssignedControl {
    assigned_control(None, 0)
}

pub(super) fn reports(control: &AssignedControl, expected: &str) -> bool {
    control
        .counts
        .iter()
        .any(|(label, count)| label.as_str() == expected && *count > 0)
}

pub(super) fn occurs(control: &AssignedControl) -> bool {
    control.counts.values().any(|count| *count > 0)
}

fn occurrences(control: &AssignedControl, labels: &[EnumLabel]) -> u64 {
    labels
        .iter()
        .filter_map(|label| control.counts.get(label))
        .fold(0, |sum, count| sum.saturating_add(*count))
}

impl Diagram {
    /// Holds where `test` accepts the selector's control. A selector outside the check has
    /// no control.
    pub(super) fn control(
        &mut self,
        controls: &Controls,
        key: &SelectorKey,
        test: impl Fn(&AssignedControl) -> bool,
    ) -> Option<Ref> {
        let Some(reader) = controls.get(key) else {
            return Some(constant(test(&empty_control())));
        };
        let emitted = self.test(
            reader.variable,
            reader
                .controls
                .iter()
                .map(|control| constant(test(control))),
        )?;
        let Some((gate, emits)) = &reader.gate else {
            return Some(emitted);
        };
        let silent = constant(test(&empty_control()));
        self.test(
            *gate,
            emits
                .iter()
                .map(|emits| if *emits { emitted } else { silent }),
        )
    }

    fn matches(
        &mut self,
        controls: &Controls,
        selector: &ControlSelector,
        (labels, count): (&[EnumLabel], u64),
    ) -> Option<Ref> {
        self.control(controls, &SelectorKey::from(selector), |control| {
            occurrences(control, labels) >= count
        })
    }

    pub(in crate::graph_verifier) fn guard(
        &mut self,
        controls: &Controls,
        guard: &Guard,
    ) -> Option<Ref> {
        self.charge(1)?;
        match guard {
            Guard::In { value, labels } => self.matches(controls, value, (labels.values(), 1)),
            Guard::KOfMap {
                count,
                value,
                labels,
            } => self.matches(controls, value, (labels.values(), count.get())),
            Guard::KOfN {
                count,
                values,
                labels,
            } => {
                self.charge(values.as_slice().len())?;
                let matching = values
                    .as_slice()
                    .iter()
                    .map(|value| self.matches(controls, value, (labels.values(), 1)))
                    .collect::<Option<Vec<_>>>()?;
                self.at_least(count.get(), &matching)
            }
            Guard::All { guards } | Guard::Any { guards } => {
                let guards = guards
                    .as_slice()
                    .iter()
                    .map(|guard| self.guard(controls, guard))
                    .collect::<Option<Vec<_>>>()?;
                if matches!(guard, Guard::All { .. }) {
                    self.all(guards)
                } else {
                    self.any(guards)
                }
            }
            Guard::Not { guard } => {
                let guard = self.guard(controls, guard)?;
                self.not(guard)
            }
        }
    }

    pub(in crate::graph_verifier) fn predicate(
        &mut self,
        controls: &Controls,
        predicate: &CompletionPredicate,
    ) -> Option<Ref> {
        self.charge(1)?;
        match predicate {
            CompletionPredicate::Always => Some(ALWAYS),
            CompletionPredicate::Never => Some(NEVER),
            CompletionPredicate::Guard(guard) => self.guard(controls, guard),
            CompletionPredicate::Shared(predicate) => self.predicate(controls, predicate),
            CompletionPredicate::CandidateGuard { guard, available } => self
                .candidate(controls, guard, available)
                .map(|(established, _)| established),
            CompletionPredicate::Not(predicate) => {
                let predicate = self.predicate(controls, predicate)?;
                self.not(predicate)
            }
            CompletionPredicate::All(predicates) => {
                let predicates = self.predicates(controls, predicates)?;
                self.all(predicates)
            }
            CompletionPredicate::Any(predicates) => {
                let predicates = self.predicates(controls, predicates)?;
                self.any(predicates)
            }
            CompletionPredicate::AtLeast { count, predicates } => {
                let predicates = self.predicates(controls, predicates)?;
                self.at_least(*count, &predicates)
            }
        }
    }

    pub(in crate::graph_verifier) fn predicates(
        &mut self,
        controls: &Controls,
        predicates: &[CompletionPredicate],
    ) -> Option<Vec<Ref>> {
        predicates
            .iter()
            .map(|predicate| self.predicate(controls, predicate))
            .collect()
    }

    /// Three-valued guard truth for one first-join candidate: where the guard is established
    /// and where it is refuted. Controls the candidate cannot see leave it undecided.
    fn candidate(
        &mut self,
        controls: &Controls,
        guard: &Guard,
        available: &BTreeMap<SelectorKey, CompletionPredicate>,
    ) -> Option<(Ref, Ref)> {
        self.charge(1)?;
        let leaf = |diagram: &mut Self, value: &ControlSelector, query: (&[EnumLabel], u64)| {
            diagram.charge(1)?;
            let key = SelectorKey::from(value);
            let present = match available.get(&key) {
                Some(presence) if controls.contains_key(&key) => {
                    diagram.predicate(controls, presence)?
                }
                _ => NEVER,
            };
            let matches = diagram.matches(controls, value, query)?;
            let differs = diagram.not(matches)?;
            Some((
                diagram.and(present, matches)?,
                diagram.and(present, differs)?,
            ))
        };
        let (count, truths) = match guard {
            Guard::In { value, labels } => return leaf(self, value, (labels.values(), 1)),
            Guard::KOfMap {
                count,
                value,
                labels,
            } => return leaf(self, value, (labels.values(), count.get())),
            Guard::Not { guard } => {
                return self
                    .candidate(controls, guard, available)
                    .map(|(established, refuted)| (refuted, established));
            }
            Guard::KOfN {
                count,
                values,
                labels,
            } => (
                count.get(),
                values
                    .as_slice()
                    .iter()
                    .map(|value| leaf(self, value, (labels.values(), 1)))
                    .collect::<Option<Vec<_>>>()?,
            ),
            Guard::All { guards } | Guard::Any { guards } => (
                if matches!(guard, Guard::All { .. }) {
                    guards.as_slice().len() as u64
                } else {
                    1
                },
                guards
                    .as_slice()
                    .iter()
                    .map(|guard| self.candidate(controls, guard, available))
                    .collect::<Option<Vec<_>>>()?,
            ),
        };
        let established = truths.iter().map(|truth| truth.0).collect::<Vec<_>>();
        let open = truths
            .iter()
            .map(|truth| self.not(truth.1))
            .collect::<Option<Vec<_>>>()?;
        let reachable = self.at_least(count, &open)?;
        Some((self.at_least(count, &established)?, self.not(reachable)?))
    }

    /// Holds where the named node reports a failure: an error, a failed join, or no signal
    /// from a node whose signals the check reads.
    pub(in crate::graph_verifier) fn failure(
        &mut self,
        controls: &Controls,
        name: &NodeName,
    ) -> Option<Ref> {
        let keys = controls
            .keys()
            .filter(|key| key.name == *name && key.source != ControlSourceKey::Execution)
            .cloned()
            .collect::<Vec<_>>();
        let mut failed = NEVER;
        let mut silent = None;
        for key in &keys {
            let reported = self.control(controls, key, |control| {
                control
                    .counts
                    .iter()
                    .any(|(label, count)| *count > 0 && control_label_is_failure(key, label))
            })?;
            failed = self.or(failed, reported)?;
            if key.source == ControlSourceKey::Signal {
                let absent = self.control(controls, key, |control| !occurs(control))?;
                silent = Some(self.and(silent.unwrap_or(ALWAYS), absent)?);
            }
        }
        self.or(failed, silent.unwrap_or(NEVER))
    }
}
