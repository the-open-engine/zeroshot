use super::*;

mod state;
use state::*;

mod map;

#[derive(Clone, Copy)]
enum ParallelControlResult {
    Reached,
    Unreachable,
}

struct GroupEvaluation<'a, G> {
    group: &'a G,
    context: &'a Context,
    traversal: Traversal<'a>,
}

impl Engine<'_> {
    pub(super) fn eval_choice(
        &mut self,
        group: &ChoiceNode,
        context: &mut Context,
        traversal: Traversal<'_>,
    ) -> Result<Status, ReducerError> {
        let mut local = context.clone();
        local.local_writes.clear();
        let selected = group
            .branches
            .as_slice()
            .iter()
            .find(|branch| {
                self.guard(&branch.when, &local, traversal.map_indices)
                    .unwrap_or(false)
            })
            .map(|branch| &branch.node)
            .or(group.otherwise.as_deref())
            .ok_or(ReducerError::MissingChoiceRoute)?;
        self.trace_branch(&group.name, selected.name());
        let result = self.eval(selected, &mut local, traversal);
        merge_runtime_facts(&local, context);
        let status = result?;
        if let Status::Continue { position } = status {
            self.promote_group(
                (&group.name, &group.promoted_state_paths),
                (&local, context),
                traversal,
            )?;
            Ok(Status::Continue { position })
        } else {
            Ok(status)
        }
    }

    pub(super) fn eval_parallel(
        &mut self,
        group: &ParNode,
        context: &mut Context,
        traversal: Traversal<'_>,
    ) -> Result<Status, ReducerError> {
        let frontier = self.pending_frontier.clone();
        let checkpoint = self.trace_checkpoint();
        let probes = self.evaluate_parallel_branches(
            group,
            context,
            Traversal {
                mode: EvalMode::Probe,
                record_evidence: false,
                ..traversal
            },
        )?;
        let probe_trace = self.take_trace_since(checkpoint);
        let (mut ordered, required) = self.parallel_completions(group, &probes, traversal)?;
        ordered.sort_by_key(|(index, position)| (*position, *index));
        if let Some(status) = self.unreached_parallel_join(
            GroupMutation {
                group,
                context: &mut *context,
                traversal,
            },
            ParallelProbeSummary {
                probes: &probes,
                completed: ordered.len(),
                required,
            },
        )? {
            if !matches!(status, Status::Pending) {
                if traversal.record_evidence {
                    // Observe only history-backed completed loops. This remains a bounded probe:
                    // unstarted tails neither acquire execution identities nor charge actual work.
                    self.evaluate_parallel_branches(
                        group,
                        context,
                        Traversal {
                            mode: EvalMode::Probe,
                            record_evidence: true,
                            ..traversal
                        },
                    )?;
                } else {
                    self.restore_trace(probe_trace)?;
                }
                self.pending_frontier = frontier;
                merge_parallel_visits(&probes, context);
            }
            return Ok(status);
        }
        self.pending_frontier = frontier;
        let winners = ordered.into_iter().take(required).collect::<Vec<_>>();
        let (mut joined, join_position, winner_set) = self.join_parallel_winners(
            GroupEvaluation {
                group,
                context: &*context,
                traversal,
            },
            &winners,
        )?;
        merge_parallel_visits(&probes, &mut joined);
        self.finish_parallel_join(
            GroupMutation {
                group,
                context,
                traversal,
            },
            ParallelJoinResult {
                branch_count: probes.len(),
                required,
                joined,
                position: join_position,
                winners: winner_set,
            },
        )
    }

    fn evaluate_parallel_branches(
        &mut self,
        group: &ParNode,
        context: &Context,
        traversal: Traversal<'_>,
    ) -> Result<Vec<(Status, Context)>, ReducerError> {
        let mut order = group
            .branches
            .as_slice()
            .iter()
            .enumerate()
            .collect::<Vec<_>>();
        order.sort_by_cached_key(|(index, branch)| {
            (self.last_service(branch, traversal.map_indices), *index)
        });
        let base = self.probe_loops.clone();
        let mut results = Vec::new();
        for (index, branch) in order {
            self.probe_loops = base.clone();
            let mut branch_context = context.clone();
            branch_context.local_writes.clear();
            let status = self.probe(
                branch,
                &mut branch_context,
                Traversal {
                    accepted_prefix: false,
                    ..traversal
                },
            )?;
            results.push((index, (status, branch_context)));
        }
        self.probe_loops = base;
        results.sort_by_key(|(index, _)| *index);
        Ok(results.into_iter().map(|(_, result)| result).collect())
    }

    fn parallel_completions(
        &self,
        group: &ParNode,
        probes: &[(Status, Context)],
        traversal: Traversal<'_>,
    ) -> Result<(Vec<(usize, HistoryPosition)>, usize), ReducerError> {
        let mut completed = Vec::new();
        for (index, (status, branch_context)) in probes.iter().enumerate() {
            let Status::Continue { position } = status else {
                continue;
            };
            let satisfies = match &group.join {
                Join::First { when } => {
                    self.candidate_guard(when, branch_context, traversal.map_indices)
                        == openengine_cluster_protocol::GuardTruth::True
                }
                Join::All {} | Join::Any {} | Join::Quorum { .. } => true,
            };
            if satisfies {
                completed.push((index, *position));
            }
        }
        let required = parallel_join_required(&group.join, probes.len())?;
        Ok((completed, required))
    }

    fn unreached_parallel_join(
        &mut self,
        evaluation: GroupMutation<'_, ParNode>,
        summary: ParallelProbeSummary<'_>,
    ) -> Result<Option<Status>, ReducerError> {
        let GroupMutation {
            group,
            context,
            traversal,
        } = evaluation;
        if summary.completed >= summary.required {
            return Ok(None);
        }
        let potential = summary
            .probes
            .iter()
            .filter(|(status, _)| matches!(status, Status::Pending))
            .count();
        let settled = summary.completed + potential < summary.required;
        if !settled {
            self.continue_parallel_candidates(GroupMutation {
                group,
                context,
                traversal,
            })?;
            return Ok(Some(Status::Pending));
        }
        let position = summary
            .probes
            .iter()
            .map(|(status, candidate)| match status {
                Status::Pending => candidate.evidence_position,
                _ => status.position().max(candidate.evidence_position),
            })
            .max()
            .unwrap_or(HistoryPosition::ZERO);
        for branch in group.branches.as_slice() {
            self.void_active_descendants(
                branch,
                VoidScope {
                    map_indices: traversal.map_indices,
                    cutoff: position,
                    reason: ExecutionVoidReason::ParallelJoin,
                    emit_decisions: traversal.mode == EvalMode::Decide,
                },
            );
        }
        self.record_parallel_control(
            GroupMutation {
                group,
                context,
                traversal,
            },
            ParallelControlResult::Unreachable,
        );
        self.continue_decision(&group.name, traversal.mode);
        Ok(Some(Status::Continue { position }))
    }

    fn continue_parallel_candidates(
        &mut self,
        evaluation: GroupMutation<'_, ParNode>,
    ) -> Result<(), ReducerError> {
        let GroupMutation {
            group,
            context,
            traversal,
        } = evaluation;
        if traversal.mode == EvalMode::Decide {
            let mut branches = group
                .branches
                .as_slice()
                .iter()
                .enumerate()
                .collect::<Vec<_>>();
            branches.sort_by_cached_key(|(index, branch)| {
                (self.last_service(branch, traversal.map_indices), *index)
            });
            for (_, branch) in branches {
                let mut local = context.clone();
                let _ = self.eval(branch, &mut local, traversal)?;
            }
        }
        Ok(())
    }

    fn join_parallel_winners(
        &mut self,
        evaluation: GroupEvaluation<'_, ParNode>,
        winners: &[(usize, HistoryPosition)],
    ) -> Result<(Context, HistoryPosition, BTreeSet<usize>), ReducerError> {
        let GroupEvaluation {
            group,
            context,
            traversal,
        } = evaluation;
        let join_position = winners
            .iter()
            .map(|(_, position)| *position)
            .max()
            .unwrap_or(HistoryPosition::ZERO);
        let winner_set = winners
            .iter()
            .map(|(index, _)| *index)
            .collect::<BTreeSet<_>>();
        let mut joined = context.clone();
        joined.local_writes.clear();
        for (index, _) in winners {
            let mut branch_context = context.clone();
            branch_context.local_writes.clear();
            let branch = group
                .branches
                .as_slice()
                .get(*index)
                .ok_or(ReducerError::InconsistentHistory)?;
            let status = self.eval(
                branch,
                &mut branch_context,
                Traversal {
                    cutoff: join_position,
                    accepted_prefix: false,
                    ..traversal
                },
            )?;
            if !matches!(status, Status::Continue { .. }) {
                return Err(ReducerError::InconsistentHistory);
            }
            for path in
                promoted_write_paths(&branch_context.local_writes, &group.promoted_state_paths)
            {
                let value = select(&branch_context.state, &path)?.clone();
                set_path(&mut joined.state, &path, value)?;
                joined.local_writes.insert(path);
            }
            let branch_names = descendant_names(branch);
            merge_scoped_runtime_facts(
                &branch_context,
                &mut joined,
                &branch_names,
                traversal.map_indices,
            );
        }
        Ok((joined, join_position, winner_set))
    }

    fn finish_parallel_join(
        &mut self,
        evaluation: GroupMutation<'_, ParNode>,
        mut join: ParallelJoinResult,
    ) -> Result<Status, ReducerError> {
        let GroupMutation {
            group,
            context,
            traversal,
        } = evaluation;
        self.emit_parallel_promotion(group, &join.joined, traversal)?;
        join.joined
            .local_writes
            .extend(context.local_writes.clone());
        *context = join.joined;
        self.record_parallel_control(
            GroupMutation {
                group,
                context,
                traversal,
            },
            ParallelControlResult::Reached,
        );
        if join.required < join.branch_count {
            for (index, branch) in group.branches.as_slice().iter().enumerate() {
                if !join.winners.contains(&index) {
                    self.void_active_descendants(
                        branch,
                        VoidScope {
                            map_indices: traversal.map_indices,
                            cutoff: join.position,
                            reason: ExecutionVoidReason::ParallelJoin,
                            emit_decisions: traversal.mode == EvalMode::Decide,
                        },
                    );
                }
            }
        }
        self.continue_decision(&group.name, traversal.mode);
        Ok(Status::Continue {
            position: join.position,
        })
    }

    fn record_parallel_control(
        &mut self,
        evaluation: GroupMutation<'_, ParNode>,
        result: ParallelControlResult,
    ) {
        let GroupMutation {
            group,
            context,
            traversal,
        } = evaluation;
        let first = matches!(group.join, Join::First { .. });
        let label = match (first, result) {
            (true, ParallelControlResult::Reached) => "satisfied",
            (true, ParallelControlResult::Unreachable) => "no_satisfier",
            (false, ParallelControlResult::Reached) => "reached",
            (false, ParallelControlResult::Unreachable) => "quorum_unreachable",
        };
        self.set_group_control(
            context,
            GroupControlUpdate {
                node: &group.name,
                field: if first { "raced" } else { "joined" },
                label,
                map_indices: traversal.map_indices,
            },
        );
    }

    fn emit_parallel_promotion(
        &mut self,
        group: &ParNode,
        joined: &Context,
        traversal: Traversal<'_>,
    ) -> Result<(), ReducerError> {
        if traversal.mode != EvalMode::Decide || group.promoted_state_paths.is_empty() {
            return Ok(());
        }
        let optional_paths = self
            .optional_promotions
            .get(&group.name)
            .ok_or(ReducerError::InconsistentHistory)?;
        let mut values = Vec::new();
        for path in &group.promoted_state_paths {
            if let Some(value) = promoted_value(
                &joined.state,
                path,
                &joined.local_writes,
                optional_paths.contains(path),
            )? {
                values.push(PromotedValue {
                    path: path.clone(),
                    value,
                });
            }
        }
        if values.is_empty() {
            return Ok(());
        }
        self.decisions.push(Decision::Promote {
            node: group.name.clone(),
            map_indices: traversal.map_indices.to_vec(),
            values,
        });
        Ok(())
    }
    // Control and voiding operations live in the control companion module.
}

fn parallel_join_required(join: &Join, branch_count: usize) -> Result<usize, ReducerError> {
    match join {
        Join::All {} => Ok(branch_count),
        Join::Any {} | Join::First { .. } => Ok(1),
        Join::Quorum { count } => {
            usize::try_from(count.get()).map_err(|_| ReducerError::IdentityOutOfRange)
        }
    }
}

fn merge_parallel_visits(probes: &[(Status, Context)], target: &mut Context) {
    for (_, candidate) in probes {
        merge_visit_markers(candidate, target);
    }
}
