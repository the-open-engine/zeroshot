use super::*;

mod state;
use state::*;

impl<'history> Engine<'history> {
    pub(super) fn eval(
        &mut self,
        node: &GraphNode,
        context: &mut Context,
        traversal: Traversal<'_>,
    ) -> Result<Status, ReducerError> {
        let group_boundary = group_boundary(node, traversal);
        let traversal = Traversal {
            boundary: traversal.boundary.or(group_boundary.as_ref()),
            ..traversal
        };
        let trace = self.trace_enter(node, traversal)?;
        let status = match node {
            GraphNode::Step(_) | GraphNode::Verifier(_) => {
                self.eval_worker_node(node, context, traversal)
            }
            GraphNode::Seq(_)
            | GraphNode::Choice(_)
            | GraphNode::Par(_)
            | GraphNode::Loop(_)
            | GraphNode::Map(_) => self.eval_group_node(node, context, traversal),
            GraphNode::Succeed(_) | GraphNode::Fail(_) => {
                self.eval_terminal_node(node, context, traversal)
            }
        }?;
        record_completed(node, context, traversal, &status);
        self.trace_finish(trace, &status, context);
        if matches!(node, GraphNode::Succeed(_) | GraphNode::Fail(_)) {
            self.trace_terminal(trace, &status)?;
        }
        Ok(status)
    }

    fn eval_worker_node(
        &mut self,
        node: &GraphNode,
        context: &mut Context,
        traversal: Traversal<'_>,
    ) -> Result<Status, ReducerError> {
        match node {
            GraphNode::Step(step) => self.eval_executable(
                ExecutableSpec {
                    name: &step.name,
                    worker: &step.worker,
                    input_bindings: &step.input_bindings,
                    write_bindings: &step.write_bindings,
                    attempt_ceiling: step.attempts,
                    verifier: false,
                },
                context,
                traversal,
            ),
            GraphNode::Verifier(verifier) => self.eval_executable(
                ExecutableSpec {
                    name: &verifier.name,
                    worker: &verifier.worker,
                    input_bindings: &verifier.input_bindings,
                    write_bindings: &verifier.write_bindings,
                    attempt_ceiling: verifier.attempts,
                    verifier: true,
                },
                context,
                traversal,
            ),
            _ => Err(ReducerError::InconsistentHistory),
        }
    }

    fn eval_group_node(
        &mut self,
        node: &GraphNode,
        context: &mut Context,
        traversal: Traversal<'_>,
    ) -> Result<Status, ReducerError> {
        match node {
            GraphNode::Seq(group) => self.eval_sequence(group, context, traversal),
            GraphNode::Choice(group) => self.eval_choice(group, context, traversal),
            GraphNode::Par(group) => self.eval_parallel(group, context, traversal),
            GraphNode::Loop(group) => self.eval_loop(group, context, traversal),
            GraphNode::Map(group) => self.eval_map(group, context, traversal),
            _ => Err(ReducerError::InconsistentHistory),
        }
    }

    fn eval_terminal_node(
        &self,
        node: &GraphNode,
        context: &Context,
        traversal: Traversal<'_>,
    ) -> Result<Status, ReducerError> {
        match node {
            GraphNode::Succeed(terminal) => {
                let output = bind_payload(&terminal.bindings, &context.state, traversal.item)?;
                Ok(Status::Terminal {
                    position: HistoryPosition::ZERO,
                    projection: TerminalProjection::Succeeded { output },
                })
            }
            GraphNode::Fail(terminal) => Ok(Status::Terminal {
                position: HistoryPosition::ZERO,
                projection: TerminalProjection::Failed {
                    context: None,
                    reason: terminal.reason.as_label().clone(),
                },
            }),
            _ => Err(ReducerError::InconsistentHistory),
        }
    }

    fn eval_sequence(
        &mut self,
        group: &openengine_cluster_protocol::SeqNode,
        context: &mut Context,
        traversal: Traversal<'_>,
    ) -> Result<Status, ReducerError> {
        let mut local = context.clone();
        local.local_writes.clear();
        let mut position = HistoryPosition::ZERO;
        for (index, child) in group.children.as_slice().iter().enumerate() {
            let accepted_prefix = traversal.accepted_prefix
                || group.children.as_slice()[index + 1..].iter().any(|later| {
                    self.item_has_history(&descendant_names(later), traversal.map_indices, &local)
                });
            let result = self.eval(
                child,
                &mut local,
                Traversal {
                    accepted_prefix,
                    ..traversal
                },
            );
            if result.is_err() {
                merge_runtime_facts(&local, context);
            }
            match result? {
                Status::Continue {
                    position: child_position,
                } => {
                    position = position.max(child_position);
                }
                other => {
                    merge_runtime_facts(&local, context);
                    return Ok(other.after(position));
                }
            }
        }
        self.promote_group(
            (&group.name, &group.promoted_state_paths),
            (&local, context),
            traversal,
        )?;
        Ok(Status::Continue { position })
    }

    fn eval_loop(
        &mut self,
        group: &openengine_cluster_protocol::LoopNode,
        context: &mut Context,
        traversal: Traversal<'_>,
    ) -> Result<Status, ReducerError> {
        let mut local = context.clone();
        let mut loop_names = descendant_names(&group.body);
        loop_names.insert(group.name.clone());
        let mut position = HistoryPosition::ZERO;
        for iteration in 1..=group.max_iterations.get() {
            clear_scoped_runtime_facts(&mut local, &loop_names, traversal.map_indices);
            // Optional outputs describe this iteration; required state still carries across rounds.
            local.local_writes.clear();
            let loop_iterations = self.enter_loop_round(group, traversal, iteration)?;
            let result = self.eval_loop_body(
                group,
                &mut local,
                Traversal {
                    loop_iterations: &loop_iterations,
                    ..traversal
                },
            );
            if result.is_err() {
                merge_runtime_facts(&local, context);
            }
            match result? {
                Status::Continue {
                    position: body_position,
                } => {
                    position = position.max(body_position);
                }
                other => {
                    merge_runtime_facts(&local, context);
                    return Ok(other.after(position));
                }
            }
            let converged = group
                .until
                .as_ref()
                .map(|guard| self.guard(guard, &local, traversal.map_indices))
                .transpose()?
                .unwrap_or(false);
            if converged {
                return self.finish_loop(LoopFinishRequest {
                    group,
                    context,
                    traversal,
                    completion: LoopCompletion::new(local, position, "converged", iteration),
                });
            }
        }
        self.finish_loop(LoopFinishRequest {
            group,
            context,
            traversal,
            completion: LoopCompletion::new(
                local,
                position,
                "exhausted",
                group.max_iterations.get(),
            ),
        })
    }

    fn enter_loop_round(
        &mut self,
        group: &openengine_cluster_protocol::LoopNode,
        traversal: Traversal<'_>,
        iteration: u64,
    ) -> Result<Vec<u64>, ReducerError> {
        let mut rounds = traversal.loop_iterations.to_vec();
        rounds.push(iteration);
        if traversal.mode != EvalMode::Probe || traversal.accepted_prefix {
            self.charge_loop(&group.name, traversal.map_indices, &rounds)?;
        }
        if traversal.mode == EvalMode::Probe {
            self.probe_loops
                .record(&group.name, traversal.map_indices, &rounds)
                .map_err(|_| ReducerError::ProbeLoopLimit)?;
        }
        Ok(rounds)
    }

    fn eval_loop_body(
        &mut self,
        group: &openengine_cluster_protocol::LoopNode,
        context: &mut Context,
        traversal: Traversal<'_>,
    ) -> Result<Status, ReducerError> {
        self.active_loops.push((
            group.name.clone(),
            traversal.map_indices.to_vec(),
            traversal.loop_iterations.to_vec(),
        ));
        let result = self.eval(&group.body, context, traversal);
        self.active_loops.pop();
        result
    }

    fn finish_loop(&mut self, request: LoopFinishRequest<'_, '_>) -> Result<Status, ReducerError> {
        let LoopFinishRequest {
            group,
            context,
            traversal,
            completion,
        } = request;
        let LoopCompletion {
            mut local,
            position,
            label,
            iteration,
        } = completion;
        let mut final_round = traversal.loop_iterations.to_vec();
        final_round.push(iteration);
        let record_evidence = traversal.record_evidence
            && (traversal.mode != EvalMode::Probe
                || self
                    .loop_entries
                    .contains(&group.name, traversal.map_indices, &final_round));
        if record_evidence {
            self.loop_evidence.exhausted_loops.retain(|prior| {
                prior.node != group.name || prior.map_indices != traversal.map_indices
            });
        }
        if record_evidence && label == "exhausted" {
            self.loop_evidence
                .record_exhausted_loop(openengine_cluster_protocol::ExhaustedLoop {
                    node: group.name.clone(),
                    completed_iterations: group.max_iterations,
                    map_indices: traversal.map_indices.to_vec(),
                });
        }
        let mut loop_names = descendant_names(&group.body);
        loop_names.insert(group.name.clone());
        clear_scoped_runtime_facts(context, &loop_names, traversal.map_indices);
        self.set_group_control(
            &mut local,
            GroupControlUpdate {
                node: &group.name,
                field: "terminated",
                label,
                map_indices: traversal.map_indices,
            },
        );
        self.promote_group(
            (&group.name, &group.promoted_state_paths),
            (&local, context),
            traversal,
        )?;
        Ok(Status::Continue { position })
    }

    pub(super) fn eval_executable(
        &mut self,
        spec: ExecutableSpec<'_>,
        context: &mut Context,
        traversal: Traversal<'_>,
    ) -> Result<Status, ReducerError> {
        let resolution = self.resolve_visit(&spec, context, traversal.map_indices)?;
        let VisitResolution::Ready(visit) = resolution else {
            return attempts_exhausted();
        };
        let visit = *visit;
        let input = bind_payload(spec.input_bindings, &context.state, traversal.item)?;
        for execution in &visit.executions {
            if execution.input != input {
                return Err(ReducerError::InconsistentHistory);
            }
            self.consumed_executions.insert(execution.execution);
        }
        if !visit.executions.is_empty() {
            self.charge_active_loops()?;
        }
        mark_visit(
            spec.name,
            traversal.map_indices,
            &mut context.controls,
            visit.number,
        );
        let Some(execution) = visit.existing else {
            return self.dispatch_missing(MissingDispatchRequest {
                spec,
                visit,
                input,
                traversal,
            });
        };
        self.finish_existing_execution(ExistingExecutionRequest {
            spec,
            context,
            traversal,
            execution,
            input,
        })
    }

    fn finish_existing_execution(
        &mut self,
        request: ExistingExecutionRequest<'_, '_, '_, '_>,
    ) -> Result<Status, ReducerError> {
        let ExistingExecutionRequest {
            spec,
            context,
            traversal,
            execution,
            input,
        } = request;
        if execution.dispatch_position > traversal.cutoff {
            return Ok(Status::Pending);
        }
        let DurableExecutionState::Settled { position, outcome } = &execution.state else {
            return Ok(Status::Pending);
        };
        if *position > traversal.cutoff {
            return Ok(Status::Pending);
        }
        if self.execution_mode == ExecutionMode::NativeV2
            && matches!(
                outcome,
                WorkerOutcome::Error {
                    code: WorkerErrorCode::Crash,
                    ..
                }
            )
            && execution.attempt < spec.attempt_ceiling
        {
            return self.dispatch_retry(RetryDispatchRequest {
                spec,
                traversal,
                execution,
                input,
            });
        }
        let writes_applied = self.apply_outcome(OutcomeApplication {
            spec: &spec,
            context: &mut *context,
            map_indices: traversal.map_indices,
            outcome,
        })?;
        if writes_applied {
            self.promote_writes(&spec, context, traversal)?;
        }
        self.continue_decision(spec.name, traversal.mode);
        Ok(Status::continuing(*position))
    }

    fn resolve_visit(
        &self,
        spec: &ExecutableSpec<'_>,
        context: &Context,
        map_indices: &[u64],
    ) -> Result<VisitResolution<'history>, ReducerError> {
        let occurrence = StructuralOccurrence {
            node: spec.name.clone(),
            map_indices: map_indices.to_vec(),
        };
        let mut matching = self
            .by_occurrence
            .get(&occurrence)
            .cloned()
            .unwrap_or_default();
        let number = next_visit(spec.name, map_indices, &context.controls);
        let (attempt, existing, executions) = match self.execution_mode {
            ExecutionMode::LegacyAttempts => {
                matching.sort_by_key(|execution| execution.attempt);
                let attempt =
                    PositiveInteger::new(number).map_err(|_| ReducerError::InconsistentHistory)?;
                if attempt.get() > spec.attempt_ceiling.get() {
                    return Ok(VisitResolution::Exhausted);
                }
                let existing = matching
                    .iter()
                    .find(|execution| execution.attempt == attempt)
                    .cloned();
                let executions = existing.iter().cloned().collect();
                (attempt, existing, executions)
            }
            ExecutionMode::NativeV2 => {
                matching.sort_by_key(|execution| execution.dispatch_position);
                let executions = native_visit_executions(&matching, number);
                let existing = executions.last().cloned();
                let attempt = existing.as_ref().map_or_else(
                    || {
                        PositiveInteger::new(INITIAL_ATTEMPT)
                            .expect("the initial attempt is positive")
                    },
                    |execution| execution.attempt,
                );
                if attempt > spec.attempt_ceiling {
                    return Err(ReducerError::InconsistentHistory);
                }
                (attempt, existing, executions)
            }
        };
        Ok(VisitResolution::Ready(Box::new(ExecutableVisit {
            occurrence,
            matching,
            executions,
            number,
            attempt,
            existing,
        })))
    }

    fn dispatch_missing(
        &mut self,
        request: MissingDispatchRequest<'_, '_, '_>,
    ) -> Result<Status, ReducerError> {
        let MissingDispatchRequest {
            spec,
            visit,
            input,
            traversal,
        } = request;
        if !self.admit_leaf(spec.name, traversal)? {
            return Ok(Status::Pending);
        }
        self.charge_active_loops()?;
        self.charge_visit(spec.name)?;
        let node_instance = match visit.matching.last() {
            Some(previous) => previous.node_instance,
            None => self.allocate_node_instance()?,
        };
        self.dispatch_visit(
            &spec,
            DispatchVisit {
                node_instance,
                occurrence: visit.occurrence,
                attempt: visit.attempt,
            },
            (input, traversal),
        )
    }

    fn dispatch_retry(
        &mut self,
        request: RetryDispatchRequest<'_, '_, '_>,
    ) -> Result<Status, ReducerError> {
        let RetryDispatchRequest {
            spec,
            traversal,
            execution: previous,
            input,
        } = request;
        if !self.admit_leaf(spec.name, traversal)? {
            return Ok(Status::Pending);
        }
        let attempt = previous
            .attempt
            .get()
            .checked_add(1)
            .ok_or(ReducerError::IdentityOutOfRange)
            .and_then(|value| {
                PositiveInteger::new(value).map_err(|_| ReducerError::IdentityOutOfRange)
            })?;
        self.dispatch_visit(
            &spec,
            DispatchVisit {
                node_instance: previous.node_instance,
                occurrence: previous.occurrence.clone(),
                attempt,
            },
            (input, traversal),
        )
    }

    fn dispatch_visit(
        &mut self,
        spec: &ExecutableSpec<'_>,
        visit: DispatchVisit,
        request: (Value, Traversal<'_>),
    ) -> Result<Status, ReducerError> {
        let (input, traversal) = request;
        self.record_boundary(spec, traversal, visit.attempt.get())?;
        let execution = self.allocate_execution()?;
        self.decisions.push(Decision::Dispatch {
            node_instance: visit.node_instance,
            execution,
            occurrence: visit.occurrence,
            attempt: visit.attempt,
            iteration: traversal.loop_iterations.last().copied().unwrap_or(0),
            worker: spec.worker.clone(),
            input,
        });
        Ok(Status::Pending)
    }

    fn record_boundary(
        &mut self,
        spec: &ExecutableSpec<'_>,
        traversal: Traversal<'_>,
        attempt: u64,
    ) -> Result<(), ReducerError> {
        let boundary = traversal
            .boundary
            .cloned()
            .unwrap_or_else(|| ExecutionBoundary {
                node: spec.name.clone(),
                map_indices: traversal.map_indices.to_vec(),
                loop_iterations: traversal.loop_iterations.to_vec(),
                attempt,
            });
        if self
            .boundary
            .as_ref()
            .is_some_and(|current| current != &boundary)
        {
            return Err(ReducerError::InconsistentHistory);
        }
        self.boundary = Some(boundary);
        Ok(())
    }

    fn admit_leaf(
        &mut self,
        node: &NodeName,
        traversal: Traversal<'_>,
    ) -> Result<bool, ReducerError> {
        if !self.pending_leaf(node, traversal) {
            return Ok(false);
        }
        self.trace_queued(node, traversal)?;
        Ok(self.admit_pending(traversal))
    }

    fn admit_pending(&mut self, traversal: Traversal<'_>) -> bool {
        if traversal.mode != EvalMode::Decide
            || traversal.cutoff != HistoryPosition::MAX
            || self.dispatch_remaining == 0
        {
            return false;
        }
        self.dispatch_remaining -= 1;
        true
    }

    fn allocate_node_instance(&mut self) -> Result<NodeInstanceId, ReducerError> {
        let allocated = NodeInstanceId::new(self.next_node_instance)
            .map_err(|_| ReducerError::IdentityOutOfRange)?;
        self.next_node_instance = self
            .next_node_instance
            .checked_add(1)
            .ok_or(ReducerError::IdentityOutOfRange)?;
        Ok(allocated)
    }

    fn allocate_execution(&mut self) -> Result<ExecutionId, ReducerError> {
        let execution =
            ExecutionId::new(self.next_execution).map_err(|_| ReducerError::IdentityOutOfRange)?;
        self.next_execution = self
            .next_execution
            .checked_add(1)
            .ok_or(ReducerError::IdentityOutOfRange)?;
        Ok(execution)
    }

    fn apply_outcome(
        &mut self,
        application: OutcomeApplication<'_, '_>,
    ) -> Result<bool, ReducerError> {
        let OutcomeApplication {
            spec,
            context,
            map_indices,
            outcome,
        } = application;
        match outcome {
            WorkerOutcome::Error { code, .. } => {
                self.set_control(
                    context,
                    ControlUpdate {
                        node: spec.name,
                        source: ControlSource::Error,
                        field: None,
                        label: code.as_str(),
                        map_indices,
                    },
                );
                return Ok(false);
            }
            WorkerOutcome::Verified { output, .. } if !spec.verifier => {
                apply_writes(
                    context,
                    WriteApplication {
                        name: spec.name,
                        output,
                        signals: None,
                        diagnostic: None,
                        bindings: spec.write_bindings,
                        map_indices,
                    },
                )?;
            }
            WorkerOutcome::Verifier {
                output,
                signals,
                diagnostic,
                ..
            } if spec.verifier => {
                for (field, label) in signals {
                    self.set_control(
                        context,
                        ControlUpdate {
                            node: spec.name,
                            source: ControlSource::Signal,
                            field: Some(field.as_str()),
                            label: label.as_str(),
                            map_indices,
                        },
                    );
                }
                apply_writes(
                    context,
                    WriteApplication {
                        name: spec.name,
                        output,
                        signals: Some(signals),
                        diagnostic: Some(diagnostic),
                        bindings: spec.write_bindings,
                        map_indices,
                    },
                )?;
            }
            _ => return Err(ReducerError::InconsistentHistory),
        }
        Ok(true)
    }

    fn promote_writes(
        &mut self,
        spec: &ExecutableSpec<'_>,
        context: &Context,
        traversal: Traversal<'_>,
    ) -> Result<(), ReducerError> {
        if traversal.mode != EvalMode::Decide || spec.write_bindings.is_empty() {
            return Ok(());
        }
        let values = spec
            .write_bindings
            .iter()
            .map(|binding| {
                Ok(PromotedValue {
                    path: binding.target.clone(),
                    value: select(&context.state, &binding.target)?.clone(),
                })
            })
            .collect::<Result<Vec<_>, ReducerError>>()?;
        self.decisions.push(Decision::Promote {
            node: spec.name.clone(),
            map_indices: traversal.map_indices.to_vec(),
            values,
        });
        Ok(())
    }
    // Choice, parallel, and map evaluation live in the groups companion module.
}

fn native_visit_executions<'history>(
    executions: &[&'history DurableExecution],
    requested_visit: u64,
) -> Vec<&'history DurableExecution> {
    let mut visit = 0;
    executions
        .iter()
        .filter_map(|execution| {
            if execution.attempt.get() == 1 {
                visit += 1;
            }
            (visit == requested_visit).then_some(*execution)
        })
        .collect()
}

fn record_completed(
    node: &GraphNode,
    context: &mut Context,
    traversal: Traversal<'_>,
    status: &Status,
) {
    if let Status::Continue { position } = status {
        context.evidence_position = context.evidence_position.max(*position);
        context
            .completed
            .insert((node.name().clone(), traversal.map_indices.to_vec()));
    }
}

fn group_boundary(node: &GraphNode, traversal: Traversal<'_>) -> Option<ExecutionBoundary> {
    if traversal.boundary.is_none() && matches!(node, GraphNode::Par(_) | GraphNode::Map(_)) {
        Some(ExecutionBoundary {
            node: node.name().clone(),
            map_indices: traversal.map_indices.to_vec(),
            loop_iterations: traversal.loop_iterations.to_vec(),
            attempt: 0,
        })
    } else {
        None
    }
}
