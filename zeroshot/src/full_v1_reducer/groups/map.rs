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
        let item_count = items.len();
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
            drop(probes);
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
        let pending = results.len() < item_count
            || results
                .iter()
                .any(|result| matches!(result.status, Status::Pending));
        let evaluation = GroupMutation {
            group,
            context,
            traversal,
        };
        if pending {
            self.mark_pending_map(evaluation, &results);
            Ok(Status::Pending)
        } else {
            self.finish_map(evaluation, &results)
        }
    }

    fn mark_pending_map(&self, evaluation: GroupMutation<'_, MapNode>, results: &[MapItemResult]) {
        let GroupMutation {
            group,
            context,
            traversal: _,
        } = evaluation;
        let names = descendant_names(&group.body);
        for item in results {
            merge_scoped_runtime_facts(&item.context, context, &names, &item.scope);
        }
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
        let base = self.probe_loops.clone();
        let body_names = descendant_names(&group.body);
        let mut results = Vec::new();
        let mut pending = 0_u64;
        for (index, item) in items.iter().enumerate() {
            let mut scope = traversal.map_indices.to_vec();
            scope.push(index as u64);
            let historical = self.item_has_history(&body_names, &scope, context);
            let retained = self.item_in_frontier(&body_names, &scope, traversal.loop_iterations);
            if !historical
                && !retained
                && (pending >= group.max_items.get()
                    || self.pending_frontier.len() >= self.frontier_limit)
            {
                continue;
            }
            if mode == EvalMode::Probe {
                self.probe_loops = base.clone();
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
            compact_map_context(&mut item_context, group, (&body_names, &scope))?;
            results.push(MapItemResult {
                probe_work: self.map_probe_work(mode, &scope),
                status,
                context: item_context,
                scope,
            });
        }
        self.probe_loops = base;
        Ok(results)
    }

    fn map_probe_work(&self, mode: EvalMode, scope: &[u64]) -> LoopWork {
        if mode == EvalMode::Probe {
            self.probe_loops.scoped(scope)
        } else {
            LoopWork::default()
        }
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
            self.probe_loops
                .merge(&selected.probes[selected.index].probe_work)
                .map_err(|_| ReducerError::ProbeLoopLimit)?;
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
                            .get(
                                *terminal_scope
                                    .last()
                                    .ok_or(ReducerError::InconsistentHistory)?
                                    as usize,
                            )
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
                self.probe_loops
                    .merge(&item.probe_work)
                    .map_err(|_| ReducerError::ProbeLoopLimit)?;
            }
        }
        Ok(())
    }
}

// Retain only item-owned facts and promoted values, never a copy of the full caller input.
fn compact_map_context(
    context: &mut Context,
    group: &MapNode,
    scope: (&BTreeSet<NodeName>, &[u64]),
) -> Result<(), ReducerError> {
    let (names, indices) = scope;
    let mut state = Value::Object(Map::new());
    let writes = promoted_write_paths(&context.local_writes, &group.promoted_state_paths);
    for path in &group.promoted_state_paths {
        if !promoted_write_paths(&context.local_writes, std::slice::from_ref(path)).is_empty() {
            set_path(&mut state, path, select(&context.state, path)?.clone())?;
        }
    }
    context.state = state;
    context.local_writes = writes.into_iter().collect();
    context
        .completed
        .retain(|(name, scope)| names.contains(name) && scope.starts_with(indices));
    context
        .channels
        .retain(|(name, scope), _| names.contains(name) && scope.starts_with(indices));
    context
        .controls
        .retain(|key, _| names.contains(&key.node) && key.map_indices.starts_with(indices));
    Ok(())
}
