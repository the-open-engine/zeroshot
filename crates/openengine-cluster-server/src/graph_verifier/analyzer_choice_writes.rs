use super::*;
use openengine_cluster_protocol::NonEmptyEnumSet;

impl Analyzer<'_> {
    pub(super) fn correlate_choice_writes(
        &mut self,
        choice: &ChoiceNode,
        alternatives: &[Effects],
        merged: &mut Effects,
    ) {
        let completing = alternatives
            .iter()
            .filter(|branch| branch.falls_through)
            .collect::<Vec<_>>();
        let Some(first) = completing.first() else {
            return;
        };
        // These are conditional execution proofs, not ordinary dominating nodes. Only a
        // route-masked error guard may use them after this choice has actually completed.
        for branch in &completing {
            for name in &branch.definite_nodes {
                if merged.definite_nodes.contains(name)
                    || !self.nodes.get(name).is_some_and(|info| {
                        matches!(info.node, GraphNode::Step(_) | GraphNode::Verifier(_))
                    })
                {
                    continue;
                }
                self.choice_execution_correlations
                    .entry(name.clone())
                    .or_default()
                    .push(ChoiceExecutionCorrelation {
                        owner: choice.name.clone(),
                        presence: branch.completion.clone(),
                    });
            }
        }
        let candidates = first
            .definite_writes
            .keys()
            .chain(
                first
                    .outcome_writes
                    .values()
                    .flat_map(|writes| writes.keys()),
            )
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut writes = Writes::new();
        let mut requirements = vec![BTreeSet::new(); completing.len()];
        for path in candidates {
            if merged.definite_writes.contains_key(&path) {
                continue;
            }
            let facts = completing
                .iter()
                .map(|branch| self.choice_path_fact(branch, &path))
                .collect::<Option<Vec<_>>>();
            let Some(facts) = facts else {
                continue;
            };
            let providers = facts.iter().map(|(write, _)| write).collect::<Vec<_>>();
            let Some(common) = intersect_write_fact_set(&providers) else {
                continue;
            };
            writes.insert(path, common);
            for (required, (_, producer)) in requirements.iter_mut().zip(facts) {
                required.extend(producer);
            }
        }
        if !writes.is_empty() {
            // The choice's synthetic outcome fact only owns common paths. A successful
            // selected branch never makes an unselected producer's private paths defined.
            // Deduplicate producer proofs across fields before expanding nested choice
            // conditions. A many-field result must not multiply the proof tree at each scope.
            let condition = CompletionPredicate::any(completing.iter().zip(requirements).map(
                |(branch, names)| {
                    CompletionPredicate::all(
                        std::iter::once(branch.completion.clone()).chain(
                            names
                                .into_iter()
                                .map(|name| self.choice_producer_success(&name)),
                        ),
                    )
                },
            ));
            self.choice_write_conditions
                .insert(choice.name.clone(), condition);
            merged.outcome_writes.insert(choice.name.clone(), writes);
            merged.outcome_order.push(choice.name.clone());
            merged.exit_failed.insert(choice.name.clone());
        }
    }

    fn choice_path_fact(
        &self,
        branch: &Effects,
        path: &FieldPath,
    ) -> Option<(WriteFact, Option<NodeName>)> {
        for name in branch.outcome_order.iter().rev() {
            let Some(writes) = branch.outcome_writes.get(name) else {
                continue;
            };
            if writes
                .keys()
                .any(|target| target != path && paths_overlap(target, path))
            {
                return None;
            }
            if let Some(write) = writes.get(path) {
                let mut common = write.clone();
                // Resolved writes leave outcome_order, so a remaining conditional
                // writer need not be the last writer of this path. Bound the fact
                // by both possibilities instead of reviving an earlier narrow type.
                for (target, definite) in &branch.definite_writes {
                    if !paths_overlap(target, path) {
                        continue;
                    }
                    if target != path {
                        return None;
                    }
                    common = intersect_write_facts(&common, definite)?;
                }
                return Some((common, Some(name.clone())));
            }
        }
        branch
            .definite_writes
            .get(path)
            .cloned()
            .map(|write| (write, None))
    }

    fn choice_producer_success(&self, name: &NodeName) -> CompletionPredicate {
        if let Some(condition) = self.choice_write_conditions.get(name) {
            return condition.clone();
        }
        let Ok(labels) = NonEmptyEnumSet::new(error_labels()) else {
            return CompletionPredicate::Never;
        };
        CompletionPredicate::not(CompletionPredicate::Guard(Guard::In {
            value: ControlSelector {
                name: name.clone(),
                source: ControlSource::Error,
                field: None,
            },
            labels,
        }))
    }

    pub(super) fn invalidate_choice_writes(&self, pending: &mut OutcomeWrites, later: &Writes) {
        for (name, writes) in pending.iter_mut() {
            if self.choice_write_conditions.contains_key(name) {
                writes.retain(|path, _| !later.keys().any(|target| paths_overlap(path, target)));
            }
        }
        pending.retain(|_, writes| !writes.is_empty());
    }

    pub(super) fn choice_error_is_available(
        &mut self,
        selector: &ControlSelector,
        available: &BTreeSet<NodeName>,
        context: (&CompletionPredicate, &[DiagnosticPathSegment]),
    ) -> bool {
        if selector.source != ControlSource::Error {
            return false;
        }
        let correlations = self
            .choice_execution_correlations
            .get(&selector.name)
            .cloned()
            .unwrap_or_default();
        for correlation in correlations {
            if !available.contains(&correlation.owner) {
                continue;
            }
            let Some(completion) = self.node_completion.get(&correlation.owner).cloned() else {
                continue;
            };
            let required = CompletionPredicate::all([completion, context.0.clone()]);
            let Some(assignments) = self.assignments_for_completion_predicates(
                &[&required, &correlation.presence],
                context.1,
            ) else {
                continue;
            };
            if assignments.iter().all(|assignment| {
                !required.evaluate(assignment) || correlation.presence.evaluate(assignment)
            }) {
                return true;
            }
        }
        false
    }
}

pub(super) fn refine_common_choice_writes(
    control: &mut ChoiceControl,
    choice: &ChoiceNode,
    assignments: &[Assignment],
    conditions: &[(NodeName, CompletionPredicate)],
) {
    let mut residuals = vec![Vec::new(); choice.branches.as_slice().len() + 1];
    for assignment in assignments {
        let route = choice
            .branches
            .as_slice()
            .iter()
            .position(|branch| evaluate_guard(&branch.when, assignment))
            .unwrap_or(choice.branches.as_slice().len());
        residuals[route].push(assignment);
    }
    for (refinement, residual) in control
        .branches
        .iter_mut()
        .chain(std::iter::once(&mut control.otherwise))
        .zip(residuals)
    {
        if residual.is_empty() {
            continue;
        }
        for (name, condition) in conditions {
            if residual
                .iter()
                .all(|assignment| condition.evaluate(assignment))
            {
                refinement.failed.remove(name);
                refinement.success.insert(name.clone());
            }
        }
    }
}

pub(super) fn guarded_selector_uses(
    guard: &Guard,
) -> Vec<(&ControlSelector, bool, CompletionPredicate)> {
    fn collect<'a>(
        guard: &'a Guard,
        condition: CompletionPredicate,
        uses: &mut Vec<(&'a ControlSelector, bool, CompletionPredicate)>,
    ) {
        match guard {
            Guard::In { value, .. } => uses.push((value, false, condition)),
            Guard::KOfMap { value, .. } => uses.push((value, true, condition)),
            Guard::KOfN { values, .. } => uses.extend(
                values
                    .as_slice()
                    .iter()
                    .map(|value| (value, false, condition.clone())),
            ),
            Guard::Not { guard } => collect(guard, condition, uses),
            Guard::All { guards } | Guard::Any { guards } => {
                let mut prefix = condition;
                for child in guards.as_slice() {
                    collect(child, prefix.clone(), uses);
                    let evaluated = CompletionPredicate::Guard(child.clone());
                    prefix = CompletionPredicate::all([
                        prefix,
                        if matches!(guard, Guard::All { .. }) {
                            evaluated
                        } else {
                            CompletionPredicate::not(evaluated)
                        },
                    ]);
                }
            }
        }
    }
    let mut uses = Vec::new();
    collect(guard, CompletionPredicate::Always, &mut uses);
    uses
}
