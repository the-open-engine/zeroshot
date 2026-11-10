use super::*;

struct MapItemsEvaluation<'a> {
    evaluation: GroupEvaluation<'a, MapNode>,
    items: &'a [Value],
}

impl Engine<'_> {
    pub(in crate::full_v1_reducer) fn eval_map(
        &mut self,
        group: &MapNode,
        context: &mut Context,
        traversal: Traversal<'_>,
    ) -> Result<Status, ReducerError> {
        let selected = select_data(&group.over, &context.state, traversal.item)?;
        let items = selected
            .as_array()
            .ok_or(ReducerError::InvalidDurableValue)?;
        self.check_map_work(group, items, (context, traversal))?;
        let checkpoint = self.trace_checkpoint();
        let probes = self.evaluate_map_items(
            MapItemsEvaluation {
                evaluation: GroupEvaluation {
                    group,
                    context: &*context,
                    traversal: Traversal {
                        record_evidence: false,
                        ..traversal
                    },
                },
                items,
            },
            EvalMode::Probe,
        )?;
        let probe_trace = self.take_trace_since(checkpoint);
        if let Some((terminal_index, terminal)) = earliest_terminal(&probes) {
            if traversal.mode == EvalMode::Probe && !traversal.record_evidence {
                let scope = &probes[terminal_index].scope;
                self.restore_trace(
                    probe_trace
                        .into_iter()
                        .filter(|record| record.map_indices.starts_with(scope))
                        .collect(),
                )?;
            }
            return self.finish_terminal_map_item(
                GroupEvaluation {
                    group,
                    context: &*context,
                    traversal,
                },
                TerminalMapSelection {
                    items,
                    probes: &probes,
                    index: terminal_index,
                    terminal,
                },
            );
        }
        let results = if traversal.mode == EvalMode::Probe && !traversal.record_evidence {
            self.restore_trace(probe_trace)?;
            probes
        } else {
            self.evaluate_map_items(
                MapItemsEvaluation {
                    evaluation: GroupEvaluation {
                        group,
                        context: &*context,
                        traversal,
                    },
                    items,
                },
                traversal.mode,
            )?
        };
        if results.len() < items.len()
            || results
                .iter()
                .any(|result| matches!(result.status, Status::Pending))
        {
            return Ok(Status::Pending);
        }
        self.finish_map(
            GroupMutation {
                group,
                context,
                traversal,
            },
            &results,
        )
    }

    fn evaluate_map_items(
        &mut self,
        evaluation: MapItemsEvaluation<'_>,
        mode: EvalMode,
    ) -> Result<Vec<MapItemResult>, ReducerError> {
        let MapItemsEvaluation {
            evaluation:
                GroupEvaluation {
                    group,
                    context,
                    traversal,
                },
            items,
        } = evaluation;
        let base = self.prospective_loops.clone();
        let mut results = Vec::new();
        let mut pending = 0_u64;
        for (index, item) in items.iter().enumerate() {
            let mut scope = traversal.map_indices.to_vec();
            scope.push(index as u64);
            let historical = self.item_has_history(&group.body, &scope, context);
            let retained = self.item_in_frontier(&group.body, &scope, traversal.loop_iterations);
            if !historical
                && !retained
                && (pending >= group.max_items.get()
                    || self.pending_frontier.len() >= self.frontier_limit)
            {
                continue;
            }
            if mode == EvalMode::Probe {
                self.prospective_loops = base.clone();
            }
            let mut item_context = context.clone();
            item_context.local_writes.clear();
            let status = self.probe(
                &group.body,
                &mut item_context,
                Traversal {
                    map_indices: &scope,
                    item: Some(item),
                    mode,
                    ..traversal
                },
            )?;
            if matches!(status, Status::Pending) {
                pending += 1;
            }
            results.push(MapItemResult {
                prospective: self.prospective_loops.clone(),
                status,
                context: item_context,
                scope,
            });
        }
        self.prospective_loops = base;
        Ok(results)
    }

    fn finish_terminal_map_item(
        &mut self,
        evaluation: GroupEvaluation<'_, MapNode>,
        selected: TerminalMapSelection<'_>,
    ) -> Result<Status, ReducerError> {
        let GroupEvaluation {
            group,
            context,
            traversal,
        } = evaluation;
        let terminal_scope = &selected
            .probes
            .get(selected.index)
            .ok_or(ReducerError::InconsistentHistory)?
            .scope;
        if traversal.mode == EvalMode::Probe {
            self.prospective_loops
                .merge(&selected.probes[selected.index].prospective)
                .map_err(|_| ReducerError::ProspectiveWorkLimit)?;
        }
        let terminal = if traversal.mode != EvalMode::Probe || traversal.record_evidence {
            let mut terminal_context = context.clone();
            self.eval(
                &group.body,
                &mut terminal_context,
                Traversal {
                    map_indices: terminal_scope,
                    item: Some(
                        selected
                            .items
                            .get(selected.index)
                            .ok_or(ReducerError::InconsistentHistory)?,
                    ),
                    cutoff: selected.terminal.position(),
                    ..traversal
                },
            )?
        } else {
            selected.terminal
        };
        self.void_map_losers(
            &group.body,
            MapVoidScope {
                common: VoidScope {
                    map_indices: traversal.map_indices,
                    cutoff: terminal.position(),
                    reason: ExecutionVoidReason::MapTerminal,
                    emit_decisions: traversal.mode == EvalMode::Decide,
                },
                winner_scope: terminal_scope,
            },
        );
        Ok(terminal)
    }

    fn finish_map(
        &mut self,
        evaluation: GroupMutation<'_, MapNode>,
        results: &[MapItemResult],
    ) -> Result<Status, ReducerError> {
        let GroupMutation {
            group,
            context,
            traversal,
        } = evaluation;
        self.merge_map_probe(traversal, results)?;
        let mut local = context.clone();
        let mut collected_paths = Vec::new();
        for path in &group.promoted_state_paths {
            // A worker error has no output write. Leave the whole collection
            // unchanged so authored outcome guards can handle that error;
            // never fabricate placeholders or silently shorten the array.
            if results.iter().any(|item| {
                promoted_write_paths(&item.context.local_writes, std::slice::from_ref(path))
                    .is_empty()
            }) {
                continue;
            }
            let values = results
                .iter()
                .map(|item| select(&item.context.state, path).cloned())
                .collect::<Result<Vec<_>, _>>()?;
            set_path(&mut local.state, path, Value::Array(values))?;
            local.local_writes.insert(path.clone());
            collected_paths.push(path.clone());
        }
        self.promote_group(
            (&group.name, &collected_paths),
            (&local, context),
            traversal,
        )?;
        let body_names = descendant_names(&group.body);
        for item in results {
            merge_scoped_runtime_facts(&item.context, context, &body_names, &item.scope);
        }
        let position = results
            .iter()
            .map(|item| item.status.position())
            .max()
            .unwrap_or(HistoryPosition::ZERO);
        Ok(Status::Continue { position })
    }
    fn merge_map_probe(
        &mut self,
        traversal: Traversal<'_>,
        results: &[MapItemResult],
    ) -> Result<(), ReducerError> {
        if traversal.mode == EvalMode::Probe {
            for item in results {
                self.prospective_loops
                    .merge(&item.prospective)
                    .map_err(|_| ReducerError::ProspectiveWorkLimit)?;
            }
        }
        Ok(())
    }
}
