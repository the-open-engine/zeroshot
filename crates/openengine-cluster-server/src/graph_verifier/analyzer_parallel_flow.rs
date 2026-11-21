use super::*;

struct ParallelMergeContext<'a> {
    incoming: &'a Flow,
    path: &'a [DiagnosticPathSegment],
}

impl<'a> Analyzer<'a> {
    fn unknown_parallel(&mut self, path: &[DiagnosticPathSegment]) -> Effects {
        self.assignment_ceiling(path);
        Effects {
            falls_through: true,
            completion: CompletionPredicate::Always,
            ..Effects::default()
        }
    }

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
        // Settling this join does not establish whether earlier workers succeeded.
        effects
            .exit_failed
            .extend(context.node.incoming.failed.iter().cloned());
        effects
    }

    fn merge_parallel(
        &mut self,
        group: &ParNode,
        branches: &[Effects],
        context: ParallelMergeContext<'_>,
    ) -> Effects {
        let required = parallel_required_completions(&group.join, branches.len());
        if required > u64::try_from(branches.len()).unwrap_or(u64::MAX) {
            // Shape validation owns the invalid quorum diagnostic. Assume continuation so
            // terminal analysis does not cascade it into a misleading reachability error.
            return Effects {
                falls_through: true,
                completion: CompletionPredicate::Always,
                ..merge_parallel_scenarios(branches, required, &[])
            };
        }
        let Some(correlation) = self.correlation_for_effects(group, branches, context.incoming)
        else {
            return self.unknown_parallel(context.path);
        };
        let qualifying = correlation.retention().branches.clone();
        self.parallel_join_correlations
            .insert(group.name.clone(), correlation);

        let mut guards = Vec::new();
        for predicate in &qualifying {
            predicate.collect_guards(&mut guards);
        }
        let Some((scenarios, unreachable)) =
            self.decide(&guards, &with_field(context.path, "join"), |proof| {
                completing_sets(proof, &qualifying, required)
            })
        else {
            // The assignment ceiling diagnostic is sufficient. Keep later passes conservative.
            return Effects {
                falls_through: true,
                completion: CompletionPredicate::Always,
                ..merge_parallel_scenarios(branches, required, &[])
            };
        };
        if scenarios.is_empty() && matches!(group.join, Join::First { .. }) {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::InvalidGraphShape,
                "first join guard cannot be established by any candidate racer",
                field_path(context.path, &["join", "when"]),
                vec![group.name.clone()]
            );
        }
        let mut merged = merge_parallel_scenarios(branches, required, &scenarios);
        if unreachable {
            merged.exit_failed.insert(group.name.clone());
        }
        // Both reached and impossible joins return normally; controls retain success eligibility.
        merged.falls_through = true;
        merged.completion = CompletionPredicate::Always;
        merged
    }

    fn correlation_for_effects(
        &self,
        group: &ParNode,
        branches: &[Effects],
        incoming: &Flow,
    ) -> Option<ParallelJoinCorrelation> {
        let completions = branches
            .iter()
            .map(|branch| branch.completion.clone())
            .collect::<Vec<_>>();
        self.parallel_correlation(group, &completions, incoming)
    }

    pub(super) fn parallel_correlation(
        &self,
        group: &ParNode,
        completions: &[CompletionPredicate],
        incoming: &Flow,
    ) -> Option<ParallelJoinCorrelation> {
        let required = parallel_required_completions(&group.join, completions.len());
        let branches = if let Join::First { when } = &group.join {
            // A wide guard repeats selectors; each racer needs every distinct use once.
            let mut seen = BTreeSet::new();
            let mut uses = guard_selector_uses(when);
            uses.retain(|(selector, aggregate)| {
                seen.insert((SelectorKey::from(*selector), *aggregate))
            });
            let shared_guard = std::sync::Arc::new(when.clone());
            completions
                .iter()
                .zip(group.branches.as_slice())
                .map(|(completion, racer)| {
                    Some(CompletionPredicate::all([
                        completion.clone(),
                        CompletionPredicate::CandidateGuard {
                            guard: shared_guard.clone(),
                            available: std::sync::Arc::new(
                                self.first_guard_available_on_branch(&uses, racer, incoming)?,
                            ),
                        },
                    ]))
                })
                .collect::<Option<Vec<_>>>()?
        } else {
            completions.to_vec()
        };
        let retention = ParallelRetention {
            branches: branches.clone(),
            required,
        };
        Some(match &group.join {
            Join::First { .. } => ParallelJoinCorrelation::First {
                satisfaction: CompletionPredicate::any(branches),
                retention,
            },
            _ => ParallelJoinCorrelation::Joined {
                completion: CompletionPredicate::at_least(required, branches),
                retention,
            },
        })
    }

    fn first_guard_available_on_branch(
        &self,
        uses: &[(&ControlSelector, bool)],
        racer: &GraphNode,
        incoming: &Flow,
    ) -> Option<BTreeMap<SelectorKey, CompletionPredicate>> {
        let mut owned = BTreeSet::new();
        collect_visible_names(racer, &mut owned);
        let mut available = BTreeMap::new();
        for (selector, aggregate) in uses.iter().copied() {
            let depth = self.map_depth(&selector.name);
            if !aggregate && depth > self.map_depth(racer.name()) {
                continue;
            }
            let owner = if aggregate && depth > self.map_depth(racer.name()) {
                self.nodes
                    .get(&selector.name)
                    .and_then(|info| {
                        info.scope_path().iter().find_map(|part| {
                            let DiagnosticPathSegment::Node { name } = part else {
                                return None;
                            };
                            (matches!(self.nodes.get(name)?.node, GraphNode::Map(_))
                                && (owned.contains(name) || incoming.available.contains(name)))
                            .then_some(name)
                        })
                    })
                    .unwrap_or(&selector.name)
            } else {
                &selector.name
            };
            let presence = if incoming.available.contains(owner) {
                self.incoming_control_presence(owner, incoming)?
            } else if owned.contains(owner) {
                self.conditional_candidate_presence(owner, racer.name())?
            } else {
                continue;
            };
            available.insert(SelectorKey::from(selector), presence);
        }
        Some(available)
    }

    fn incoming_control_presence(
        &self,
        name: &NodeName,
        incoming: &Flow,
    ) -> Option<CompletionPredicate> {
        let mut predicates = Vec::new();
        for (owner, nodes) in &incoming.conditional_nodes {
            if nodes.contains(name) {
                predicates.push(self.conditional_candidate_presence(name, owner)?);
            }
        }
        Some(CompletionPredicate::all(predicates))
    }

    pub(super) fn conditional_candidate_presence(
        &self,
        target: &NodeName,
        racer: &NodeName,
    ) -> Option<CompletionPredicate> {
        let path = self.nodes.get(target)?.scope_path();
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
            if let Some(presence) =
                self.candidate_parent_presence(self.nodes.get(*name)?.node, &names)?
            {
                guards.push(presence);
            }
        }
        Some(CompletionPredicate::all(guards))
    }
}

impl Analyzer<'_> {
    fn candidate_choice_route(
        &self,
        choice: &ChoiceNode,
        names: &[&NodeName],
    ) -> Option<CompletionPredicate> {
        let mut routes = self.choice_routes.borrow_mut();
        if !routes.contains_key(&choice.name) {
            let mut predicates = choice_branch_completion_predicates(choice);
            predicates.push(choice_otherwise_completion_predicate(choice));
            routes.insert(
                choice.name.clone(),
                predicates
                    .into_iter()
                    .map(|predicate| CompletionPredicate::Shared(std::sync::Arc::new(predicate)))
                    .collect(),
            );
        }
        let index = choice
            .branches
            .as_slice()
            .iter()
            .position(|branch| names.contains(&branch.node.name()))
            .unwrap_or(choice.branches.as_slice().len());
        routes.get(&choice.name)?.get(index).cloned()
    }

    fn candidate_parent_presence(
        &self,
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
            GraphNode::Choice(choice) => Some(Some(self.candidate_choice_route(choice, names)?)),
            _ => Some(None),
        }
    }
}

/// Every set of branches that can qualify together with at least `required` members, and
/// whether some legal outcome qualifies fewer. Promotion guarantees depend on the order of
/// these sets, so they are listed; the list is bounded by the join's own branches, and
/// writing it out is search work like any other.
fn completing_sets(
    proof: &mut ControlProof,
    qualifying: &[CompletionPredicate],
    required: u64,
) -> Option<(Vec<BTreeSet<usize>>, bool)> {
    let qualifying = qualifying
        .iter()
        .map(|predicate| proof.predicate(predicate))
        .collect::<Option<Vec<_>>>()?;
    let enough = proof.diagram.at_least(required, &qualifying)?;
    let short = proof.diagram.not(enough)?;
    let unreachable = proof.possible(short)?;
    // A branch that always qualifies is in every set and one that never does is in none,
    // so only the others split the outcomes.
    let (settled, undecided): (Vec<_>, Vec<_>) = qualifying
        .iter()
        .copied()
        .enumerate()
        .partition(|(_, branch)| matches!(*branch, ALWAYS | NEVER));
    let always = settled
        .iter()
        .filter_map(|(branch, qualifies)| (*qualifies == ALWAYS).then_some(*branch))
        .collect::<Vec<_>>();
    let mut sets = Vec::new();
    // The undecided branches that qualify in the outcomes being split.
    let mut members = Vec::new();
    // Legal outcomes still open, the next branch to decide, and how many qualified so far.
    let mut open = vec![(proof.diagram.and(proof.legal, enough)?, 0, 0)];
    while let Some((outcomes, next, qualified)) = open.pop() {
        if outcomes == NEVER {
            continue;
        }
        members.truncate(qualified);
        let Some((branch, qualifies)) = undecided.get(next).copied() else {
            // Listing a set copies its members; four members weigh one search step.
            proof
                .diagram
                .charge(1 + (always.len() + members.len()) / 4)?;
            sets.push(always.iter().chain(&members).copied().collect());
            if sets.len() as u64 > FULL_V1_MAX_GUARD_ASSIGNMENTS {
                return None;
            }
            continue;
        };
        proof.diagram.charge(1)?;
        let failed = proof.diagram.not(qualifies)?;
        open.push((proof.diagram.and(outcomes, failed)?, next + 1, qualified));
        // Taken next, so the branch is still the last member when its outcomes are split.
        open.push((
            proof.diagram.and(outcomes, qualifies)?,
            next + 1,
            qualified + 1,
        ));
        members.push(branch);
    }
    sets.sort();
    Some((sets, unreachable))
}
