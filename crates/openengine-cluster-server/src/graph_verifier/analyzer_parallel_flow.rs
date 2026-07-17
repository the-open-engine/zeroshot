use super::*;

struct ParallelMergeContext<'a> {
    incoming: &'a Flow,
    path: &'a [DiagnosticPathSegment],
}

impl<'a> Analyzer<'a> {
    pub(super) fn validate_par(
        &mut self,
        group: &ParNode,
        context: &LocatedNodeValidationContext<'_>,
    ) -> Effects {
        self.validate_join(group, context.node.incoming, &context.path);
        let branches = group
            .branches
            .as_slice()
            .iter()
            .map(|branch| {
                self.validate_node(
                    branch,
                    NodeValidationContext {
                        incoming: context.node.incoming,
                        state: &group.state,
                        ..context.node
                    },
                )
            })
            .collect::<Vec<_>>();
        self.validate_parallel_writes(group, &branches, &context.path);
        let mut effects = self.merge_parallel(
            group,
            &branches,
            ParallelMergeContext {
                incoming: context.node.incoming,
                path: &context.path,
            },
        );
        let success_only_nodes = effects.definite_nodes.clone();
        effects.definite_nodes.insert(group.name.clone());
        effects
            .conditional_nodes
            .entry(group.name.clone())
            .or_default()
            .extend(success_only_nodes);
        let rule = if matches!(group.join, Join::All {}) {
            PromotionRule::Definite
        } else {
            PromotionRule::EveryAlternative
        };
        self.restrict_promotions(PromotionValidationContext {
            group_state: &group.state,
            enclosing_state: context.node.state,
            promoted: &group.promoted_state_paths,
            effects: &mut effects,
            path: &context.path,
            rule,
            target_overrides: context.node.map_index_targets,
        });
        let directly_owned_writes = writes_without_parallel_owners(
            &effects.definite_writes,
            &effects.parallel_definition_effects,
        );
        if let Some(definition_effect) = parallel_definition_effect(
            &group.name,
            &context.node.incoming.defined,
            &directly_owned_writes,
        ) {
            effects.parallel_definition_effects.push(definition_effect);
        }
        effects.exit_failed.insert(group.name.clone());
        effects
    }

    fn merge_parallel(
        &mut self,
        group: &ParNode,
        branches: &[Effects],
        context: ParallelMergeContext<'_>,
    ) -> Effects {
        if matches!(group.join, Join::First { .. }) {
            return self.merge_first_parallel(group, branches, context);
        }
        let join = &group.join;
        let required = parallel_required_completions(join, branches.len());
        if required > branches.len() as u64 {
            // Shape validation owns the invalid quorum diagnostic. Assume continuation so
            // terminal analysis does not cascade it into a misleading reachability error.
            return Effects {
                falls_through: true,
                completion: CompletionPredicate::Always,
                ..merge_parallel_scenarios(branches, required, &[])
            };
        }
        let completion = CompletionPredicate::at_least(
            required,
            branches
                .iter()
                .map(|branch| branch.completion.clone())
                .collect(),
        );
        self.parallel_join_correlations.insert(
            group.name.clone(),
            ParallelJoinCorrelation::Joined {
                completion: completion.clone(),
            },
        );

        let predicates = branches
            .iter()
            .map(|branch| &branch.completion)
            .collect::<Vec<_>>();
        let Some(assignments) = self
            .assignments_for_completion_predicates(&predicates, &with_field(context.path, "join"))
        else {
            // The assignment ceiling diagnostic is sufficient. Keep later passes conservative.
            return Effects {
                falls_through: true,
                completion: CompletionPredicate::Always,
                ..merge_parallel_scenarios(branches, required, &[])
            };
        };
        let scenarios = assignments
            .iter()
            .filter_map(|assignment| {
                let completing = branches
                    .iter()
                    .enumerate()
                    .filter_map(|(index, branch)| {
                        branch.completion.evaluate(assignment).then_some(index)
                    })
                    .collect::<BTreeSet<_>>();
                (completing.len() as u64 >= required).then_some(completing)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut merged = merge_parallel_scenarios(branches, required, &scenarios);
        merged.completion = completion;
        merged
    }

    fn merge_first_parallel(
        &mut self,
        group: &ParNode,
        branches: &[Effects],
        context: ParallelMergeContext<'_>,
    ) -> Effects {
        let Join::First { when } = &group.join else {
            unreachable!("merge_first_parallel requires a first join")
        };
        let satisfiers = branches
            .iter()
            .map(|branch| {
                if self.first_guard_available_on_branch(when, branch, context.incoming) {
                    CompletionPredicate::all([
                        branch.completion.clone(),
                        CompletionPredicate::Guard(when.clone()),
                    ])
                } else {
                    CompletionPredicate::Never
                }
            })
            .collect::<Vec<_>>();
        let satisfaction = CompletionPredicate::any(satisfiers.clone());
        let all_completions = CompletionPredicate::all(
            branches
                .iter()
                .map(|branch| branch.completion.clone())
                .collect::<Vec<_>>(),
        );
        self.parallel_join_correlations.insert(
            group.name.clone(),
            ParallelJoinCorrelation::First {
                satisfaction: satisfaction.clone(),
                all_completions: all_completions.clone(),
            },
        );
        let settlement = CompletionPredicate::any([satisfaction.clone(), all_completions.clone()]);
        let predicates = vec![&satisfaction, &all_completions];
        let Some(assignments) = self
            .assignments_for_completion_predicates(&predicates, &with_field(context.path, "join"))
        else {
            return Effects {
                falls_through: true,
                completion: CompletionPredicate::Always,
                ..merge_parallel_scenarios(branches, 1, &[])
            };
        };
        let scenarios = assignments
            .iter()
            .filter_map(|assignment| {
                let satisfying = satisfiers
                    .iter()
                    .enumerate()
                    .filter_map(|(index, predicate)| {
                        predicate.evaluate(assignment).then_some(index)
                    })
                    .collect::<BTreeSet<_>>();
                (!satisfying.is_empty()).then_some(satisfying)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut merged = merge_parallel_scenarios(branches, 1, &scenarios);
        merged.falls_through = assignments
            .iter()
            .any(|assignment| settlement.evaluate(assignment));
        merged.completion = settlement;
        merged
    }

    fn first_guard_available_on_branch(
        &self,
        when: &Guard,
        branch: &Effects,
        incoming: &Flow,
    ) -> bool {
        guard_selector_uses(when)
            .into_iter()
            .all(|(selector, map_aggregate)| {
                incoming.available.contains(&selector.name)
                    || branch.definite_nodes.contains(&selector.name)
                    || (map_aggregate
                        && self
                            .map_owner(&selector.name)
                            .is_some_and(|owner| branch.definite_nodes.contains(owner)))
            })
    }
}
