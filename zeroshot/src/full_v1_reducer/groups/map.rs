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
            self.mark_pending_map(evaluation, &results)?;
            Ok(Status::Pending)
        } else {
            self.finish_map(evaluation, &results)
        }
    }

    fn mark_pending_map(
        &self,
        evaluation: GroupMutation<'_, MapNode>,
        results: &[MapItemResult],
    ) -> Result<(), ReducerError> {
        let GroupMutation {
            group,
            context,
            traversal: _,
        } = evaluation;
        let names = descendant_names(&group.body);
        for item in results {
            merge_scoped_runtime_facts(&item.context, context, &names, &item.scope)?;
        }
        Ok(())
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
        let body_names = descendant_names(&group.body);
        let mut results = Vec::new();
        let mut pending = 0_u64;
        let mut queued = 0_usize;
        for (index, item) in items.iter().enumerate() {
            let mut scope = traversal.map_indices.to_vec();
            scope.push(index as u64);
            let historical = self.item_has_history(&body_names, &scope, traversal);
            let retained = self.item_in_frontier(&body_names, &scope, traversal.loop_iterations);
            if !historical
                && !retained
                && (pending >= group.max_items.get()
                    || queued >= self.frontier_limit
                    || self.pending_frontier.len() >= self.frontier_limit)
            {
                continue;
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
                queued += usize::from(!historical);
            }
            compact_map_context(&mut item_context, group, (&body_names, &scope))?;
            results.push(MapItemResult {
                status,
                context: item_context,
                scope,
            });
        }
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
        // Replay every started item through the same terminal boundary. Their execution
        // progress is independent of which item's values and controls survive.
        let window = Traversal {
            cutoff: selected.terminal.position().max(traversal.after),
            ..traversal
        };
        let checkpoint = self.trace_checkpoint();
        self.evaluate_map_items(
            MapItemsEvaluation {
                evaluation: GroupEvaluation {
                    group,
                    context,
                    traversal: Traversal {
                        mode: if traversal.mode == EvalMode::Probe {
                            EvalMode::Probe
                        } else {
                            EvalMode::Replay
                        },
                        record_evidence: false,
                        ..window
                    },
                },
                items: selected.items,
            },
            if traversal.mode == EvalMode::Probe {
                EvalMode::Probe
            } else {
                EvalMode::Replay
            },
        )?;
        drop(self.take_trace_since(checkpoint));
        let mut terminal_context = context.clone();
        let terminal = self.eval(
            &group.body,
            &mut terminal_context,
            Traversal {
                map_indices: terminal_scope,
                item: Some(
                    &selected.items[*terminal_scope
                        .last()
                        .ok_or(ReducerError::InconsistentHistory)?
                        as usize],
                ),
                ..window
            },
        )?;
        if traversal.mode != EvalMode::Probe {
            self.confirm_work(&terminal_context, &group.name)?;
        }
        self.void_map_losers(
            &group.body,
            MapVoidScope {
                common: VoidScope {
                    map_indices: traversal.map_indices,
                    cutoff: terminal.position(),
                    reason: ExecutionVoidReason::MapTerminal,
                    mode: traversal.mode,
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
            merge_scoped_runtime_facts(&item.context, context, &body_names, &item.scope)?;
        }
        let position = results
            .iter()
            .map(|item| item.status.position())
            .max()
            .unwrap_or(HistoryPosition::ZERO);
        Ok(Status::Continue { position })
    }
}

// Retain only item-owned facts and promoted values, never a copy of the full caller input.
fn compact_map_context(
    context: &mut Context,
    group: &MapNode,
    scope: (&BTreeSet<NodeName>, &[u64]),
) -> Result<(), ReducerError> {
    let (names, indices) = scope;
    context.work.retain_scope(indices);
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
