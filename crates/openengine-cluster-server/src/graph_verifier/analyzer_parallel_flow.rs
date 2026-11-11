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
            return self.merge_first_parallel(
                group,
                branches,
                ParallelMergeContext {
                    incoming: context.incoming,
                    path: context.path,
                },
            );
        }
        let join = &group.join;
        let required = parallel_required_completions(join, branches.len());
        if required > u64::try_from(branches.len()).unwrap_or(u64::MAX) {
            // Shape validation owns the invalid quorum diagnostic. Assume continuation so
            // terminal analysis does not cascade it into a misleading reachability error.
            return Effects {
                falls_through: true,
                completion: CompletionPredicate::Always,
                ..merge_parallel_scenarios(branches, required, &[])
            };
        }
        let correlation = self.correlation_for_effects(group, branches, context.incoming);
        self.parallel_join_correlations
            .insert(group.name.clone(), correlation);

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
                (u64::try_from(completing.len()).unwrap_or(u64::MAX) >= required)
                    .then_some(completing)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut merged = merge_parallel_scenarios(branches, required, &scenarios);
        // Both reached and impossible joins return normally; controls retain success eligibility.
        merged.completion = CompletionPredicate::Always;
        merged
    }

    fn merge_first_parallel(
        &mut self,
        group: &ParNode,
        branches: &[Effects],
        context: ParallelMergeContext<'_>,
    ) -> Effects {
        let correlation = self.correlation_for_effects(group, branches, context.incoming);
        let satisfiers = correlation.retention().branches.clone();
        let satisfaction = CompletionPredicate::any(satisfiers.clone());
        self.parallel_join_correlations
            .insert(group.name.clone(), correlation);
        let predicates = vec![&satisfaction];
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
        if scenarios.is_empty() {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::InvalidGraphShape,
                "first join guard cannot be established by any candidate racer",
                field_path(context.path, &["join", "when"]),
                vec![group.name.clone()]
            );
        }
        let mut merged = merge_parallel_scenarios(branches, 1, &scenarios);
        merged.falls_through = true;
        merged.completion = CompletionPredicate::Always;
        merged
    }

    fn correlation_for_effects(
        &self,
        group: &ParNode,
        branches: &[Effects],
        incoming: &Flow,
    ) -> ParallelJoinCorrelation {
        let completions = branches
            .iter()
            .map(|branch| branch.completion.clone())
            .collect::<Vec<_>>();
        self.parallel_correlation(group, &completions, &incoming.available)
    }

    pub(super) fn parallel_correlation(
        &self,
        group: &ParNode,
        completions: &[CompletionPredicate],
        incoming: &BTreeSet<NodeName>,
    ) -> ParallelJoinCorrelation {
        let required = parallel_required_completions(&group.join, completions.len());
        let branches = if let Join::First { when } = &group.join {
            completions
                .iter()
                .zip(group.branches.as_slice())
                .map(|(completion, racer)| {
                    CompletionPredicate::all([
                        completion.clone(),
                        CompletionPredicate::CandidateGuard {
                            guard: when.clone(),
                            available: self.first_guard_available_on_branch(when, racer, incoming),
                        },
                    ])
                })
                .collect::<Vec<_>>()
        } else {
            completions.to_vec()
        };
        let retention = ParallelRetention {
            branches: branches.clone(),
            required,
        };
        match &group.join {
            Join::First { .. } => ParallelJoinCorrelation::First {
                satisfaction: CompletionPredicate::any(branches),
                retention,
            },
            _ => ParallelJoinCorrelation::Joined {
                completion: CompletionPredicate::at_least(required, branches),
                retention,
            },
        }
    }

    pub(super) fn first_guard_available_on_branch(
        &self,
        when: &Guard,
        racer: &GraphNode,
        incoming: &BTreeSet<NodeName>,
    ) -> BTreeMap<SelectorKey, CompletionPredicate> {
        let mut owned = BTreeSet::new();
        collect_names(racer, &mut owned);
        guard_selector_uses(when)
            .into_iter()
            .filter_map(|(selector, aggregate)| {
                let owner = if aggregate {
                    self.nodes
                        .get(&selector.name)
                        .and_then(|info| {
                            info.path.iter().find_map(|part| {
                                let DiagnosticPathSegment::Node { name } = part else {
                                    return None;
                                };
                                (matches!(self.nodes.get(name)?.node, GraphNode::Map(_))
                                    && (owned.contains(name) || incoming.contains(name)))
                                .then_some(name)
                            })
                        })
                        .unwrap_or(&selector.name)
                } else {
                    &selector.name
                };
                let presence = if incoming.contains(owner) {
                    CompletionPredicate::Always
                } else if owned.contains(owner) {
                    self.conditional_candidate_presence(owner, racer.name())?
                } else {
                    return None;
                };
                Some((SelectorKey::from(selector), presence))
            })
            .collect()
    }

    pub(super) fn conditional_candidate_presence(
        &self,
        target: &NodeName,
        racer: &NodeName,
    ) -> Option<CompletionPredicate> {
        let path = &self.nodes.get(target)?.path;
        let names = path
            .iter()
            .filter_map(|segment| match segment {
                DiagnosticPathSegment::Node { name } => Some(name),
                _ => None,
            })
            .collect::<Vec<_>>();
        let start = names.iter().position(|name| *name == racer)?;
        let mut guards = Vec::new();
        for name in names[start..].iter().filter(|name| **name != target) {
            if let Some(presence) = candidate_parent_presence(self.nodes.get(*name)?.node, &names)?
            {
                guards.push(presence);
            }
        }
        Some(CompletionPredicate::all(guards))
    }
}

fn candidate_choice_route(choice: &ChoiceNode, names: &[&NodeName]) -> Option<CompletionPredicate> {
    let branch = choice
        .branches
        .as_slice()
        .iter()
        .position(|branch| names.contains(&branch.node.name()));
    match branch {
        Some(index) => choice_branch_completion_predicates(choice)
            .get(index)
            .cloned(),
        None => Some(choice_otherwise_completion_predicate(choice)),
    }
}

fn candidate_parent_presence(
    node: &GraphNode,
    names: &[&NodeName],
) -> Option<Option<CompletionPredicate>> {
    match node {
        GraphNode::Par(group) => {
            let index = group
                .branches
                .as_slice()
                .iter()
                .position(|branch| names.contains(&branch.name()))?;
            Some(Some(retention_guard(group, index)?))
        }
        GraphNode::Choice(choice) => Some(Some(candidate_choice_route(choice, names)?)),
        _ => Some(None),
    }
}
