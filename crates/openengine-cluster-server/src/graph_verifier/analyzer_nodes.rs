use super::*;

impl<'a> Analyzer<'a> {
    pub(super) fn validate_node(
        &mut self,
        node: &GraphNode,
        context: NodeValidationContext<'_>,
    ) -> Effects {
        let path = self.node_path(node.name());
        let located = LocatedNodeValidationContext {
            node: context,
            path,
        };
        self.validate_group_state(node, &located);
        let effects = match node {
            GraphNode::Step(step) => self.validate_step(step, &located),
            GraphNode::Verifier(verifier) => self.validate_verifier(verifier, &located),
            GraphNode::Seq(group) => self.validate_seq(group, &located),
            GraphNode::Choice(group) => self.validate_choice_node(group, &located),
            GraphNode::Par(group) => self.validate_par(group, &located),
            GraphNode::Loop(group) => self.validate_loop(group, &located),
            GraphNode::Map(group) => self.validate_map(group, &located),
            GraphNode::Succeed(terminal) => self.validate_succeed(terminal, &located),
            GraphNode::Fail(terminal) => terminal_effects(&terminal.name, context.incoming),
        };
        self.node_fallthrough
            .insert(node_identity(node), effects.falls_through);
        effects
    }

    pub(super) fn validate_step(
        &mut self,
        step: &StepNode,
        context: &LocatedNodeValidationContext<'_>,
    ) -> Effects {
        self.validate_executable(ExecutableValidationContext {
            name: &step.name,
            input: &step.input,
            output: &step.output,
            input_bindings: &step.input_bindings,
            write_bindings: &step.write_bindings,
            node: context.node,
            path: &context.path,
        })
    }

    pub(super) fn validate_verifier(
        &mut self,
        verifier: &VerifierNode,
        context: &LocatedNodeValidationContext<'_>,
    ) -> Effects {
        self.validate_executable(ExecutableValidationContext {
            name: &verifier.name,
            input: &verifier.input,
            output: &verifier.output,
            input_bindings: &verifier.input_bindings,
            write_bindings: &verifier.write_bindings,
            node: context.node,
            path: &context.path,
        })
    }

    pub(super) fn validate_succeed(
        &mut self,
        terminal: &openengine_cluster_protocol::SucceedNode,
        context: &LocatedNodeValidationContext<'_>,
    ) -> Effects {
        self.validate_input_bindings(
            &terminal.bindings,
            InputBindingsValidationContext {
                target_payload: &terminal.output,
                node: context.node,
                node_path: &context.path,
                field: "bindings",
            },
        );
        terminal_effects(&terminal.name, context.node.incoming)
    }

    pub(super) fn validate_group_state(
        &mut self,
        node: &GraphNode,
        context: &LocatedNodeValidationContext<'_>,
    ) {
        if node_state(node).is_some_and(|state| {
            !is_subtype_with_definitions(context.node.state, state, &context.node.incoming.defined)
        }) {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::SchemaSafety,
                "incoming state is not a subtype of the group's declared state",
                with_field(&context.path, "state"),
                vec![node.name().clone()],
            );
        }
    }

    pub(super) fn validate_seq(
        &mut self,
        group: &SeqNode,
        context: &LocatedNodeValidationContext<'_>,
    ) -> Effects {
        let mut flow = context.node.incoming.clone();
        let mut effects = Effects::default();
        let mut falls_through = true;
        let mut completion = CompletionPredicate::Always;
        for child in group.children.as_slice() {
            let child_effects = self.validate_node(
                child,
                NodeValidationContext {
                    incoming: &flow,
                    state: &group.state,
                    item: context.node.item,
                },
            );
            if falls_through {
                flow.apply_effects(&child_effects);
            }
            completion = CompletionPredicate::all([completion, child_effects.completion.clone()]);
            falls_through = falls_through
                && child_effects.falls_through
                && self
                    .completion_is_satisfiable(&completion, &context.path)
                    .unwrap_or(true);
            effects
                .definite_nodes
                .extend(child_effects.definite_nodes.clone());
            effects
                .definite_writes
                .extend(child_effects.definite_writes.clone());
            effects
                .possible_writes
                .extend(child_effects.possible_writes);
            merge_possible_write_types(
                &mut effects.possible_write_types,
                &child_effects.possible_write_types,
            );
            merge_outcome_writes(&mut effects.outcome_writes, &child_effects.outcome_writes);
            if falls_through {
                effects.resolve_successful_writes(&flow.failed);
            }
        }
        effects.exit_failed = flow.failed;
        effects.falls_through = falls_through;
        effects.completion = completion;
        effects.definite_nodes.insert(group.name.clone());
        self.restrict_promotions(PromotionValidationContext {
            group_state: &group.state,
            enclosing_state: context.node.state,
            promoted: &group.promoted_state_paths,
            effects: &mut effects,
            path: &context.path,
            rule: PromotionRule::Definite,
        });
        effects
    }

    pub(super) fn validate_choice_node(
        &mut self,
        group: &ChoiceNode,
        context: &LocatedNodeValidationContext<'_>,
    ) -> Effects {
        let control = self.validate_choice(group, context.node.incoming, &context.path);
        self.choice_reachability
            .insert(group.name.clone(), control.reachability());
        let mut alternatives = Vec::new();
        for (index, branch) in group.branches.as_slice().iter().enumerate() {
            if !control.branch_reachable[index] {
                continue;
            }
            let mut branch_flow = context.node.incoming.clone();
            control.branches[index].apply(&mut branch_flow);
            let mut effects = self.validate_node(
                &branch.node,
                NodeValidationContext {
                    incoming: &branch_flow,
                    state: &group.state,
                    item: context.node.item,
                },
            );
            self.restrict_completion(
                &mut effects,
                control.branch_completion[index].clone(),
                &context.path,
            );
            alternatives.push(effects);
        }
        match &group.otherwise {
            Some(otherwise) if control.otherwise_reachable => {
                let mut otherwise_flow = context.node.incoming.clone();
                control.otherwise.apply(&mut otherwise_flow);
                let mut effects = self.validate_node(
                    otherwise,
                    NodeValidationContext {
                        incoming: &otherwise_flow,
                        state: &group.state,
                        item: context.node.item,
                    },
                );
                self.restrict_completion(
                    &mut effects,
                    control.otherwise_completion.clone(),
                    &context.path,
                );
                alternatives.push(effects);
            }
            _ if !control.exhaustive => {
                alternatives.push(Effects {
                    exit_failed: context.node.incoming.failed.clone(),
                    falls_through: true,
                    completion: control.otherwise_completion.clone(),
                    ..Effects::default()
                });
            }
            _ => {}
        }
        let mut effects = merge_alternatives(&alternatives);
        effects.definite_nodes.insert(group.name.clone());
        self.restrict_promotions(PromotionValidationContext {
            group_state: &group.state,
            enclosing_state: context.node.state,
            promoted: &group.promoted_state_paths,
            effects: &mut effects,
            path: &context.path,
            rule: PromotionRule::EveryAlternative,
        });
        effects
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
                        item: context.node.item,
                    },
                )
            })
            .collect::<Vec<_>>();
        self.validate_parallel_writes(group, &branches, &context.path);
        let mut effects = self.merge_parallel(&group.join, &branches, &context.path);
        effects.definite_nodes.insert(group.name.clone());
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
        });
        effects
    }

    pub(super) fn merge_parallel(
        &mut self,
        join: &Join,
        branches: &[Effects],
        path: &[DiagnosticPathSegment],
    ) -> Effects {
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

        let predicates = branches
            .iter()
            .map(|branch| &branch.completion)
            .collect::<Vec<_>>();
        let Some(assignments) =
            self.assignments_for_completion_predicates(&predicates, &with_field(path, "join"))
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
        merged.completion = CompletionPredicate::at_least(
            required,
            branches
                .iter()
                .map(|branch| branch.completion.clone())
                .collect(),
        );
        merged
    }

    pub(super) fn validate_loop(
        &mut self,
        group: &LoopNode,
        context: &LocatedNodeValidationContext<'_>,
    ) -> Effects {
        if group.max_iterations.get() > FULL_V1_MAX_LOOP_ITERATIONS {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::CeilingExceeded,
                format!("maxIterations exceeds full-v1 limit {FULL_V1_MAX_LOOP_ITERATIONS}"),
                with_field(&context.path, "maxIterations"),
                vec![group.name.clone()],
            );
        }
        let mut effects = self.validate_node(
            &group.body,
            NodeValidationContext {
                incoming: context.node.incoming,
                state: &group.state,
                item: context.node.item,
            },
        );
        self.validate_loop_exit(group, context, &effects);
        effects.definite_nodes.insert(group.name.clone());
        self.restrict_promotions(PromotionValidationContext {
            group_state: &group.state,
            enclosing_state: context.node.state,
            promoted: &group.promoted_state_paths,
            effects: &mut effects,
            path: &context.path,
            rule: PromotionRule::Definite,
        });
        effects
    }

    pub(super) fn validate_loop_exit(
        &mut self,
        group: &LoopNode,
        context: &LocatedNodeValidationContext<'_>,
        body: &Effects,
    ) {
        let mut available = context.node.incoming.available.clone();
        available.extend(body.definite_nodes.clone());
        let until_path = with_field(&context.path, "until");
        let valid = self.validate_guard(
            &group.until,
            &available,
            GuardValidationContext {
                path: &until_path,
                code: GraphDiagnosticCode::LoopExitSatisfiability,
            },
        );
        for selector in guard_selectors(&group.until) {
            let guaranteed = body.definite_nodes.contains(&selector.name)
                && self
                    .nodes
                    .get(&selector.name)
                    .is_some_and(|info| matches!(info.node, GraphNode::Verifier(_)));
            if !guaranteed {
                emit_diagnostic!(
                    self,
                    GraphDiagnosticCode::LoopExitSatisfiability,
                    format!(
                        "loop exit selector {} is not a verifier guaranteed in every iteration",
                        selector.name
                    ),
                    until_path.clone(),
                    vec![selector.name.clone(), group.name.clone()],
                );
            }
        }
        if valid && !self.guard_has_satisfying_assignment(&group.until, &until_path) {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::LoopExitSatisfiability,
                "loop exit guard is unsatisfiable",
                until_path,
                vec![group.name.clone()],
            );
        }
    }

    pub(super) fn validate_map(
        &mut self,
        group: &MapNode,
        context: &LocatedNodeValidationContext<'_>,
    ) -> Effects {
        if group.max_items.get() > FULL_V1_MAX_MAP_ITEMS {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::CeilingExceeded,
                format!("maxItems exceeds full-v1 limit {FULL_V1_MAX_MAP_ITEMS}"),
                with_field(&context.path, "maxItems"),
                vec![group.name.clone()],
            );
        }
        let item_type = self.validate_map_selector(group, context);
        let body = self.validate_node(
            &group.body,
            NodeValidationContext {
                incoming: context.node.incoming,
                state: &group.state,
                item: item_type.as_ref(),
            },
        );
        let mut effects = Effects {
            definite_nodes: BTreeSet::from([group.name.clone()]),
            possible_writes: body.possible_writes.clone(),
            possible_write_types: body.possible_write_types.clone(),
            exit_failed: context
                .node
                .incoming
                .failed
                .union(&body.exit_failed)
                .cloned()
                .collect(),
            falls_through: true,
            completion: CompletionPredicate::Always,
            ..Effects::default()
        };
        for promoted in &group.promoted_state_paths {
            if is_required_path(&group.state, promoted)
                && path_type(&group.state, promoted).is_some()
            {
                if let Some(write) = body.possible_writes.get(promoted) {
                    effects
                        .definite_writes
                        .insert(promoted.clone(), write.clone());
                }
            }
        }
        self.restrict_promotions(PromotionValidationContext {
            group_state: &group.state,
            enclosing_state: context.node.state,
            promoted: &group.promoted_state_paths,
            effects: &mut effects,
            path: &context.path,
            rule: PromotionRule::Map,
        });
        effects
    }
}
