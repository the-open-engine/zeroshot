use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openengine_cluster_protocol::{
    diff_compiled_graphs, ApplyResult, Cursor, DispatchState, Generation, OperationalStatus, Phase,
    RequestFingerprint, RunId, StopMode, StopResult, UpdateResult,
};
use openengine_cluster_server::admission::{
    AdmissionSnapshot, AdmissionStore, CancellationObserver, CancellationSignal, CommitProposal,
    ControlJournal, ControlSnapshot, IdempotencyRecord, StoreError as AdmissionStoreError,
    VerifiedIoLedger, VerifiedSeed,
};
use openengine_cluster_server::lifecycle::{
    CompletionResult, DispatchPermit, LeaseId, LifecycleEvent, LifecycleRecord, LifecycleSnapshot,
    LifecycleStore, MutationReceipt as ProtocolMutationReceipt, StopProposal, TurnId,
    UpdateProposal, VerifiedCompletion, VerifiedTurn, VoidTurn,
};
use super::fold::{LedgerPhase, PublicClusterState};
use super::record::{
    ApplyMutationReceipt, MutationKind, RecordPayload, StopMutationReceipt, TerminalOutcome,
    UpdateMutationReceipt,
};
use super::{
    admission_manifest, ClusterLedger, DispatchRequest, ExecutionId, IdempotencyId, LedgerError,
    LedgerGeneration, LedgerRunId, MutationIdentity, NodeInstanceId, Position, ReceiptPosition,
};

#[derive(Clone)]
pub struct LedgerAdapters {
    ledger: Arc<ClusterLedger>,
    cancellations: Arc<Mutex<BTreeMap<ExecutionId, CancellationSignal>>>,
}

impl LedgerAdapters {
    #[must_use]
    pub fn new(ledger: Arc<ClusterLedger>) -> Self {
        Self {
            ledger,
            cancellations: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    #[must_use]
    pub fn ledger(&self) -> &Arc<ClusterLedger> {
        &self.ledger
    }

    async fn load(&self) -> Result<LoadedAggregate, AdmissionStoreError> {
        let (prefix, state) = self.ledger.load_prefix().await.map_err(map_error)?;
        let admission = admission_snapshot(&state)?;
        let lifecycle = lifecycle_snapshot(&state, &prefix.records)?;
        Ok(LoadedAggregate {
            state,
            admission,
            lifecycle,
        })
    }

    fn cancel_all_dispatches(&self) -> Result<(), AdmissionStoreError> {
        let cancellations = self
            .cancellations
            .lock()
            .map_err(|_| AdmissionStoreError::Internal("cancellation lock poisoned".into()))?;
        for cancellation in cancellations.values() {
            cancellation.cancel();
        }
        Ok(())
    }

    async fn map_dispatch_failure(&self, error: LedgerError) -> AdmissionStoreError {
        if let Ok(state) = self.ledger.replay().await {
            if let Ok(status) = operational_status(&state) {
                if status.dispatch_state != DispatchState::Active {
                    return AdmissionStoreError::DispatchDenied {
                        current: status.dispatch_state,
                    };
                }
            }
        }
        map_error(error)
    }

    async fn completion_context(
        &self,
        lease_id: &LeaseId,
    ) -> Result<(ExecutionId, String), AdmissionStoreError> {
        let execution_id =
            ExecutionId::new(lease_id.as_str()).map_err(|_| AdmissionStoreError::UnknownLease)?;
        let state = self.ledger.replay().await.map_err(map_error)?;
        if state.voided_dispatches.contains_key(&execution_id) {
            return Err(AdmissionStoreError::CompletionRejected);
        }
        let turn_id = completion_turn_id(&state, &execution_id)?;
        if self.dispatch_is_cancelled(&execution_id)? {
            return Err(AdmissionStoreError::CompletionRejected);
        }
        Ok((execution_id, turn_id))
    }

    fn dispatch_is_cancelled(
        &self,
        execution_id: &ExecutionId,
    ) -> Result<bool, AdmissionStoreError> {
        Ok(self
            .cancellations
            .lock()
            .map_err(|_| AdmissionStoreError::Internal("cancellation lock poisoned".into()))?
            .get(execution_id)
            .is_some_and(CancellationSignal::is_cancelled))
    }

    async fn settle_completion(
        &self,
        execution_id: ExecutionId,
        output: serde_json::Value,
    ) -> Result<super::SettlementReceipt, AdmissionStoreError> {
        let key = IdempotencyId::new(format!("settlement:{}", execution_id.as_str()))
            .map_err(|_| AdmissionStoreError::SchemaViolation("execution id too long".into()))?;
        let value = serde_json::json!({
            "executionId": execution_id.as_str(),
            "output": output,
        });
        let mutation = MutationIdentity::for_value(key, "settle", &value).map_err(map_error)?;
        let settlement = self
            .ledger
            .settle(super::SettlementRequest {
                execution_id: execution_id.clone(),
                output: value["output"].clone(),
                mutation,
            })
            .await;
        self.resolve_settlement(execution_id, settlement).await
    }

    async fn resolve_settlement(
        &self,
        execution_id: ExecutionId,
        settlement: Result<super::SettlementReceipt, LedgerError>,
    ) -> Result<super::SettlementReceipt, AdmissionStoreError> {
        match settlement {
            Ok(receipt) if receipt.accepted => Ok(receipt),
            Ok(_) => Err(AdmissionStoreError::CompletionRejected),
            Err(error) => {
                let latest = self.ledger.replay().await.map_err(map_error)?;
                if latest.phase == LedgerPhase::Terminal
                    || latest.voided_dispatches.contains_key(&execution_id)
                {
                    Err(AdmissionStoreError::CompletionRejected)
                } else {
                    Err(map_error(error))
                }
            }
        }
    }
}

struct LoadedAggregate {
    state: PublicClusterState,
    admission: AdmissionSnapshot,
    lifecycle: LifecycleSnapshot,
}

impl ReceiptPosition for ApplyMutationReceipt {
    fn set_position(&mut self, position: Position) {
        self.at_position = position;
    }

    fn mark_deduped(&mut self) {
        self.result.deduped = true;
    }
}

impl ReceiptPosition for UpdateMutationReceipt {
    fn set_position(&mut self, position: Position) {
        self.at_position = position;
        self.result.at_cursor = cursor(position);
    }

    fn mark_deduped(&mut self) {
        self.result.deduped = true;
    }
}

impl ReceiptPosition for StopMutationReceipt {
    fn set_position(&mut self, position: Position) {
        self.at_position = position;
        self.result.at_cursor = cursor(position);
    }

    fn mark_deduped(&mut self) {
        self.result.deduped = true;
    }
}

#[async_trait]
impl ControlJournal for LedgerAdapters {
    async fn read_control(&self) -> Result<ControlSnapshot, AdmissionStoreError> {
        Ok(self.load().await?.admission.control)
    }

    async fn lookup_idempotency(
        &self,
        key: &openengine_cluster_protocol::IdempotencyKey,
    ) -> Result<Option<IdempotencyRecord>, AdmissionStoreError> {
        let loaded = self.load().await?;
        let key = IdempotencyId::new(key.as_str()).map_err(|_| {
            AdmissionStoreError::SchemaViolation("invalid idempotency key".to_owned())
        })?;
        let Some(receipt) = loaded.state.mutation_receipts.get(&key) else {
            return Ok(None);
        };
        let fingerprint = fingerprint_from_bytes(receipt.fingerprint)?;
        let protocol_receipt = match receipt.method.as_str() {
            "apply" => {
                let receipt: ApplyMutationReceipt = serde_json::from_slice(&receipt.value)
                    .map_err(|_| AdmissionStoreError::Internal("corrupt apply receipt".into()))?;
                ProtocolMutationReceipt::Apply(receipt.result)
            }
            "update" => {
                let receipt: UpdateMutationReceipt = serde_json::from_slice(&receipt.value)
                    .map_err(|_| AdmissionStoreError::Internal("corrupt update receipt".into()))?;
                ProtocolMutationReceipt::Update(receipt.result)
            }
            "stop" => {
                let receipt: StopMutationReceipt = serde_json::from_slice(&receipt.value)
                    .map_err(|_| AdmissionStoreError::Internal("corrupt stop receipt".into()))?;
                ProtocolMutationReceipt::Stop(receipt.result)
            }
            _ => return Ok(None),
        };
        Ok(Some(IdempotencyRecord {
            fingerprint,
            receipt: protocol_receipt,
        }))
    }
}

#[async_trait]
impl VerifiedIoLedger for LedgerAdapters {
    async fn read_verified_seed(
        &self,
        run_id: &RunId,
    ) -> Result<Option<VerifiedSeed>, AdmissionStoreError> {
        let snapshot = self.load().await?.admission;
        Ok(snapshot.seed.filter(|seed| seed.run_id == *run_id))
    }
}

#[async_trait]
impl AdmissionStore for LedgerAdapters {
    async fn read_snapshot(&self) -> Result<AdmissionSnapshot, AdmissionStoreError> {
        Ok(self.load().await?.admission)
    }

    async fn read_aggregate(
        &self,
    ) -> Result<(AdmissionSnapshot, LifecycleSnapshot), AdmissionStoreError> {
        let loaded = self.load().await?;
        Ok((loaded.admission, loaded.lifecycle))
    }

    async fn commit(
        &self,
        proposal: CommitProposal,
        cancellation: &CancellationSignal,
    ) -> Result<ApplyResult, AdmissionStoreError> {
        let state = self.ledger.replay().await.map_err(map_error)?;
        let mutation = mutation_identity(&proposal.idempotency_key, &proposal.fingerprint)?;
        if let Some(mut receipt) = self
            .ledger
            .replay_receipt::<ApplyMutationReceipt>(&state, "apply", &mutation)
            .map_err(map_error)?
        {
            receipt.result.deduped = true;
            return Ok(receipt.result);
        }
        enforce_generation(proposal.if_generation, state.generation())?;
        if cancellation.is_cancelled() {
            return Err(AdmissionStoreError::Cancelled);
        }
        let prepared = prepare_apply(&state, proposal, &mutation)?;
        let result = ApplyResult {
            generation: Some(protocol_generation(prepared.generation)?),
            run_id: Some(RunId::new(prepared.run_id.as_str())),
            phase: Phase::Running,
            deduped: false,
            diff: prepared.diff,
        };
        let committed = self
            .ledger
            .append_mutation_guarded(
                &state,
                MutationKind::Apply,
                mutation,
                prepared.payloads,
                ApplyMutationReceipt {
                    result,
                    at_position: Position::ZERO,
                },
                || {
                    if cancellation.is_cancelled() {
                        Err(LedgerError::Cancelled)
                    } else {
                        Ok(())
                    }
                },
            )
            .await
            .map_err(map_error)?;
        Ok(committed.result)
    }
}

#[async_trait]
impl LifecycleStore for LedgerAdapters {
    async fn read_lifecycle_snapshot(&self) -> Result<LifecycleSnapshot, AdmissionStoreError> {
        Ok(self.load().await?.lifecycle)
    }

    async fn update_lifecycle(
        &self,
        proposal: UpdateProposal,
    ) -> Result<UpdateResult, AdmissionStoreError> {
        proposal
            .params
            .validate()
            .map_err(|error| AdmissionStoreError::SchemaViolation(error.into()))?;
        let state = self.ledger.replay().await.map_err(map_error)?;
        let mutation = mutation_identity(&proposal.params.idempotency_key, &proposal.fingerprint)?;
        if let Some(mut receipt) = self
            .ledger
            .replay_receipt::<UpdateMutationReceipt>(&state, "update", &mutation)
            .map_err(map_error)?
        {
            receipt.result.deduped = true;
            return Ok(receipt.result);
        }
        enforce_generation(Some(proposal.params.if_generation), state.generation())?;
        if state.phase != LedgerPhase::Running || state.stop_mode.is_some() {
            return Err(AdmissionStoreError::InvalidPhase {
                current: protocol_phase(state.phase),
            });
        }
        let mut next = state.clone();
        if let Some(labels) = proposal.params.labels.clone() {
            next.labels = Some(labels);
        }
        if let Some(log_level) = proposal.params.log_level {
            next.log_level = Some(log_level);
        }
        if let Some(suspended) = proposal.params.suspended {
            next.suspended = suspended;
        }
        let result = UpdateResult {
            generation: protocol_generation(
                next.generation()
                    .ok_or_else(|| AdmissionStoreError::Internal("generation absent".into()))?,
            )?,
            run_id: RunId::new(
                next.run_id()
                    .ok_or_else(|| AdmissionStoreError::Internal("run id absent".into()))?
                    .as_str(),
            ),
            phase: Phase::Running,
            operational: operational_status(&next)?,
            at_cursor: cursor(Position::ZERO),
            deduped: false,
        };
        let committed = self
            .ledger
            .append_mutation(
                &state,
                MutationKind::Update,
                mutation,
                vec![RecordPayload::LifecycleUpdate {
                    labels: proposal.params.labels,
                    log_level: proposal.params.log_level,
                    suspended: proposal.params.suspended,
                }],
                UpdateMutationReceipt {
                    result,
                    at_position: Position::ZERO,
                },
            )
            .await
            .map_err(map_error)?;
        Ok(committed.result)
    }

    async fn stop_lifecycle(
        &self,
        proposal: StopProposal,
    ) -> Result<StopResult, AdmissionStoreError> {
        let state = self.ledger.replay().await.map_err(map_error)?;
        let mutation = mutation_identity(&proposal.params.idempotency_key, &proposal.fingerprint)?;
        if let Some(mut receipt) = self
            .ledger
            .replay_receipt::<StopMutationReceipt>(&state, "stop", &mutation)
            .map_err(map_error)?
        {
            receipt.result.deduped = true;
            if receipt.result.effective_mode == StopMode::Force {
                self.cancel_all_dispatches()?;
            }
            return Ok(receipt.result);
        }
        let prepared = prepare_stop(&state, proposal)?;
        let effective_mode = prepared.result.effective_mode;
        let committed = self
            .ledger
            .append_mutation(
                &state,
                MutationKind::Stop,
                mutation,
                prepared.payloads,
                StopMutationReceipt {
                    result: prepared.result,
                    at_position: Position::ZERO,
                },
            )
            .await;
        if effective_mode == StopMode::Force {
            let committed_force = committed.is_ok()
                || self.ledger.replay().await.is_ok_and(|state| {
                    state.phase == LedgerPhase::Terminal && state.stop_mode == Some(StopMode::Force)
                });
            if committed_force {
                self.cancel_all_dispatches()?;
            }
        }
        let committed = committed.map_err(map_error)?;
        Ok(committed.result)
    }

    async fn acquire_dispatch(
        &self,
        turn_id: TurnId,
    ) -> Result<DispatchPermit, AdmissionStoreError> {
        let state = self.ledger.replay().await.map_err(map_error)?;
        let dispatch_state = operational_status(&state)?.dispatch_state;
        if dispatch_state != DispatchState::Active {
            return Err(AdmissionStoreError::DispatchDenied {
                current: dispatch_state,
            });
        }
        let admitted = state
            .admitted
            .as_ref()
            .ok_or_else(|| AdmissionStoreError::Internal("admitted state absent".into()))?;
        let key = IdempotencyId::new(format!(
            "dispatch:{}:{}",
            admitted.generation.get(),
            turn_id.as_str()
        ))
        .map_err(|_| AdmissionStoreError::SchemaViolation("turn id is too long".into()))?;
        let value = serde_json::json!({
            "generation": admitted.generation.get(),
            "runId": admitted.run_id.as_str(),
            "turnId": turn_id.as_str()
        });
        let mutation = MutationIdentity::for_value(key, "dispatch", &value).map_err(map_error)?;
        let receipt = match self
            .ledger
            .dispatch(DispatchRequest {
                turn_id: turn_id.as_str().to_owned(),
                mutation,
            })
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => return Err(self.map_dispatch_failure(error).await),
        };
        let cancellation = self
            .cancellations
            .lock()
            .map_err(|_| AdmissionStoreError::Internal("cancellation lock poisoned".into()))?
            .entry(receipt.execution_id.clone())
            .or_default()
            .clone();
        let latest = self.ledger.replay().await.map_err(map_error)?;
        let latest_dispatch_state = operational_status(&latest)?.dispatch_state;
        if !latest.active_dispatches.contains_key(&receipt.execution_id) {
            cancellation.cancel();
            return Err(AdmissionStoreError::DispatchDenied {
                current: latest_dispatch_state,
            });
        }
        Ok(DispatchPermit {
            lease_id: LeaseId::new(receipt.execution_id.as_str()),
            turn_id,
            cancellation: cancellation.observer(),
            at_cursor: cursor(receipt.at_position),
        })
    }

    async fn complete_dispatch(
        &self,
        completion: VerifiedCompletion,
    ) -> Result<CompletionResult, AdmissionStoreError> {
        let (execution_id, turn_id) = self.completion_context(&completion.lease_id).await?;
        let receipt = self
            .settle_completion(execution_id.clone(), completion.output)
            .await?;
        self.cancellations
            .lock()
            .map_err(|_| AdmissionStoreError::Internal("cancellation lock poisoned".into()))?
            .remove(&execution_id);
        Ok(CompletionResult {
            turn_id: TurnId::new(turn_id),
            at_cursor: cursor(receipt.at_position),
            terminalized: receipt.terminalized,
        })
    }
}

fn completion_turn_id(
    state: &PublicClusterState,
    execution_id: &ExecutionId,
) -> Result<String, AdmissionStoreError> {
    state
        .active_dispatches
        .get(execution_id)
        .map(|dispatch| dispatch.turn_id.clone())
        .or_else(|| {
            state
                .settlements
                .get(execution_id)
                .map(|settlement| settlement.turn_id.clone())
        })
        .ok_or(AdmissionStoreError::UnknownLease)
}

struct PreparedApply {
    payloads: Vec<RecordPayload>,
    generation: LedgerGeneration,
    run_id: LedgerRunId,
    diff: Option<openengine_cluster_protocol::GraphDiff>,
}

fn prepare_apply(
    state: &PublicClusterState,
    proposal: CommitProposal,
    mutation: &MutationIdentity,
) -> Result<PreparedApply, AdmissionStoreError> {
    if !matches!(state.phase, LedgerPhase::Empty | LedgerPhase::Running) {
        return Err(AdmissionStoreError::InvalidPhase {
            current: protocol_phase(state.phase),
        });
    }
    let unchanged = apply_is_unchanged(state, &proposal)?;
    validate_apply_input(state, &proposal, unchanged)?;
    if unchanged {
        let admitted = state
            .admitted
            .as_ref()
            .ok_or_else(|| AdmissionStoreError::Internal("admitted state absent".into()))?;
        return Ok(PreparedApply {
            payloads: Vec::new(),
            generation: admitted.generation,
            run_id: admitted.run_id.clone(),
            diff: None,
        });
    }
    prepare_new_run(state, proposal, mutation)
}

fn apply_is_unchanged(
    state: &PublicClusterState,
    proposal: &CommitProposal,
) -> Result<bool, AdmissionStoreError> {
    state
        .admitted
        .as_ref()
        .map(|admitted| {
            admitted.compiled_ir.identity().and_then(|identity| {
                proposal
                    .compiled_ir
                    .identity()
                    .map(|desired| identity == desired)
            })
        })
        .transpose()
        .map_err(|_| AdmissionStoreError::Internal("canonical graph failed".into()))
        .map(|value| value.unwrap_or(false))
}

fn validate_apply_input(
    state: &PublicClusterState,
    proposal: &CommitProposal,
    unchanged: bool,
) -> Result<(), AdmissionStoreError> {
    if unchanged && proposal.input.is_some() {
        return Err(AdmissionStoreError::SchemaViolation(
            "unchanged apply must omit input".into(),
        ));
    }
    if !unchanged && proposal.input.is_none() {
        return Err(AdmissionStoreError::SchemaViolation(
            "apply that starts a run requires input".into(),
        ));
    }
    if !unchanged
        && state.phase == LedgerPhase::Running
        && (state.suspended || !state.active_dispatches.is_empty())
    {
        return Err(AdmissionStoreError::InvalidPhase {
            current: Phase::Running,
        });
    }
    Ok(())
}

fn prepare_new_run(
    state: &PublicClusterState,
    proposal: CommitProposal,
    mutation: &MutationIdentity,
) -> Result<PreparedApply, AdmissionStoreError> {
    let generation = state
        .generation()
        .map_or_else(|| LedgerGeneration::new(1), LedgerGeneration::checked_next)
        .map_err(|_| AdmissionStoreError::Internal("generation overflow".into()))?;
    let request = super::AdmissionRequest {
        graph: proposal.graph.clone(),
        compiled_ir: proposal.compiled_ir.clone(),
        input: proposal.input.clone().expect("changed input checked"),
        deadline: super::AbsoluteDeadline::from_unix_millis(i64::MAX as u64)
            .expect("maximum SQLite deadline is valid"),
        mutation: mutation.clone(),
    };
    let manifest = admission_manifest(&request).map_err(map_error)?;
    let run_id = super::validation::run_id(generation, &manifest.graph_digest)
        .map_err(|_| AdmissionStoreError::Internal("run identity failed".into()))?;
    let diff = diff_compiled_graphs(
        state.admitted.as_ref().map(|run| &run.compiled_ir),
        &proposal.compiled_ir,
    )
    .map_err(|_| AdmissionStoreError::Internal("graph diff failed".into()))?;
    Ok(PreparedApply {
        payloads: vec![RecordPayload::Admission {
            generation,
            run_id: run_id.clone(),
            graph: Box::new(proposal.graph),
            compiled_ir: Box::new(proposal.compiled_ir),
            input: proposal.input.expect("changed input checked"),
            manifest,
        }],
        generation,
        run_id,
        diff: Some(diff),
    })
}

struct PreparedStop {
    payloads: Vec<RecordPayload>,
    result: StopResult,
}

fn prepare_stop(
    state: &PublicClusterState,
    proposal: StopProposal,
) -> Result<PreparedStop, AdmissionStoreError> {
    enforce_generation(Some(proposal.params.if_generation), state.generation())?;
    if state.phase != LedgerPhase::Running {
        return Err(AdmissionStoreError::InvalidPhase {
            current: protocol_phase(state.phase),
        });
    }
    let accepted_mode = proposal.params.mode;
    let effective_mode = effective_stop_mode(state.stop_mode, accepted_mode)?;
    let terminal = effective_mode == StopMode::Force || state.active_dispatches.is_empty();
    let payloads = stop_payloads(state, accepted_mode, effective_mode, terminal);
    let next = projected_stop_state(state, effective_mode, terminal);
    Ok(PreparedStop {
        payloads,
        result: StopResult {
            generation: protocol_generation(
                next.generation()
                    .ok_or_else(|| AdmissionStoreError::Internal("generation absent".into()))?,
            )?,
            run_id: RunId::new(
                next.run_id()
                    .ok_or_else(|| AdmissionStoreError::Internal("run id absent".into()))?
                    .as_str(),
            ),
            phase: protocol_phase(next.phase),
            accepted_mode,
            effective_mode,
            operational: operational_status(&next)?,
            at_cursor: cursor(Position::ZERO),
            deduped: false,
        },
    })
}

fn effective_stop_mode(
    current: Option<StopMode>,
    requested: StopMode,
) -> Result<StopMode, AdmissionStoreError> {
    match (current, requested) {
        (None, mode) => Ok(mode),
        (Some(StopMode::Drain), StopMode::Force) => Ok(StopMode::Force),
        (Some(mode), requested) if mode == requested => Ok(mode),
        _ => Err(AdmissionStoreError::InvalidPhase {
            current: Phase::Running,
        }),
    }
}

fn stop_payloads(
    state: &PublicClusterState,
    accepted_mode: StopMode,
    effective_mode: StopMode,
    terminal: bool,
) -> Vec<RecordPayload> {
    let mut payloads = vec![RecordPayload::StopRequested {
        accepted_mode,
        effective_mode,
    }];
    if effective_mode == StopMode::Force {
        payloads.extend(
            state
                .active_dispatches
                .keys()
                .cloned()
                .map(|execution_id| RecordPayload::Void { execution_id }),
        );
    }
    if terminal {
        payloads.push(RecordPayload::Terminal {
            outcome: TerminalOutcome::Stopped,
        });
    }
    payloads
}

fn projected_stop_state(
    state: &PublicClusterState,
    effective_mode: StopMode,
    terminal: bool,
) -> PublicClusterState {
    let mut next = state.clone();
    next.stop_mode = Some(effective_mode);
    next.suspended = true;
    if terminal {
        next.phase = LedgerPhase::Terminal;
        next.terminal_outcome = Some(TerminalOutcome::Stopped);
        next.active_dispatches.clear();
    }
    next
}

fn admission_snapshot(
    state: &PublicClusterState,
) -> Result<AdmissionSnapshot, AdmissionStoreError> {
    let Some(admitted) = &state.admitted else {
        return Ok(AdmissionSnapshot::default());
    };
    let generation = protocol_generation(admitted.generation)?;
    let run_id = RunId::new(admitted.run_id.as_str());
    let admission_position = state
        .admission_position
        .ok_or_else(|| AdmissionStoreError::Internal("admission position is absent".into()))?;
    let control = ControlSnapshot {
        spec: Some(admitted.graph.clone()),
        compiled_ir: Some(admitted.compiled_ir.clone()),
        generation: Some(generation),
        run_id: Some(run_id.clone()),
        phase: protocol_phase(state.phase),
        cursor: Some(cursor(state.at_position)),
    };
    Ok(AdmissionSnapshot {
        control,
        seed: Some(VerifiedSeed {
            run_id,
            input: admitted.input.clone(),
            cursor: cursor(admission_position),
        }),
    })
}

fn lifecycle_snapshot(
    state: &PublicClusterState,
    records: &[super::record::LedgerRecord],
) -> Result<LifecycleSnapshot, AdmissionStoreError> {
    if state.admitted.is_none() {
        return Ok(LifecycleSnapshot::default());
    }
    let admission_position = state
        .admission_position
        .expect("admitted state always has an admission position");
    let mut projection = LifecycleProjection::default();
    for record in records {
        if record.sequence < admission_position {
            continue;
        }
        projection.apply_record(record, state.stop_mode)?;
    }
    Ok(LifecycleSnapshot {
        operational: Some(operational_status(state)?),
        latest_cursor: Some(cursor(state.at_position)),
        records: projection.records,
        verified_turns: projection.verified_turns,
        void_turns: projection.void_turns,
    })
}

#[derive(Default)]
struct LifecycleProjection {
    records: Vec<LifecycleRecord>,
    dispatch_turns: BTreeMap<ExecutionId, String>,
    verified_turns: Vec<VerifiedTurn>,
    void_turns: Vec<VoidTurn>,
    settled: BTreeSet<ExecutionId>,
}

impl LifecycleProjection {
    fn apply_record(
        &mut self,
        record: &super::record::LedgerRecord,
        stop_mode: Option<StopMode>,
    ) -> Result<(), AdmissionStoreError> {
        let payload = record
            .decode_payload()
            .map_err(|_| AdmissionStoreError::Internal("corrupt ledger record".into()))?;
        let event = match payload {
            RecordPayload::Dispatch {
                execution_id,
                turn_id,
                ..
            } => {
                self.dispatch_turns.insert(execution_id, turn_id.clone());
                Some(LifecycleEvent::Dispatched {
                    turn_id: TurnId::new(turn_id),
                })
            }
            RecordPayload::Settlement {
                execution_id,
                output,
            } => {
                if !self.settled.insert(execution_id.clone()) {
                    return Ok(());
                }
                let turn_id = self
                    .dispatch_turns
                    .get(&execution_id)
                    .cloned()
                    .ok_or_else(|| {
                        AdmissionStoreError::Internal("settlement turn absent".into())
                    })?;
                self.verified_turns.push(VerifiedTurn {
                    turn_id: TurnId::new(turn_id.clone()),
                    output,
                    cursor: cursor(record.sequence),
                });
                Some(LifecycleEvent::Verified {
                    turn_id: TurnId::new(turn_id),
                })
            }
            RecordPayload::Void { execution_id } => {
                let turn_id = self
                    .dispatch_turns
                    .get(&execution_id)
                    .cloned()
                    .ok_or_else(|| AdmissionStoreError::Internal("void turn absent".into()))?;
                self.void_turns.push(VoidTurn {
                    turn_id: TurnId::new(turn_id.clone()),
                    cursor: cursor(record.sequence),
                });
                Some(LifecycleEvent::Void {
                    turn_id: TurnId::new(turn_id),
                })
            }
            RecordPayload::LifecycleUpdate {
                labels,
                log_level,
                suspended,
            } => Some(LifecycleEvent::Updated {
                labels,
                log_level,
                suspended,
            }),
            RecordPayload::StopRequested {
                accepted_mode,
                effective_mode,
            } => Some(LifecycleEvent::StopRequested {
                accepted_mode,
                effective_mode,
            }),
            RecordPayload::Terminal { .. } => {
                stop_mode.map(|mode| LifecycleEvent::Finished { mode })
            }
            RecordPayload::Admission { .. }
            | RecordPayload::SafeFault { .. }
            | RecordPayload::EffectIntent { .. }
            | RecordPayload::EffectReceipt { .. }
            | RecordPayload::CleanupReceipt { .. }
            | RecordPayload::MutationReceipt { .. } => None,
        };
        if let Some(event) = event {
            self.records.push(LifecycleRecord {
                cursor: cursor(record.sequence),
                event,
            });
        }
        Ok(())
    }
}

fn operational_status(
    state: &PublicClusterState,
) -> Result<OperationalStatus, AdmissionStoreError> {
    let in_flight = u32::try_from(state.active_dispatches.len())
        .map_err(|_| AdmissionStoreError::Internal("in-flight overflow".into()))?;
    let dispatch_state = match state.phase {
        LedgerPhase::Empty => DispatchState::Stopped,
        LedgerPhase::Terminal => DispatchState::Stopped,
        LedgerPhase::Running => match state.stop_mode {
            Some(StopMode::Drain) => DispatchState::Draining,
            Some(StopMode::Force) => DispatchState::ForceStopping,
            None if state.suspended => DispatchState::Suspended,
            None => DispatchState::Active,
        },
    };
    Ok(OperationalStatus {
        labels: state.labels.clone().unwrap_or_default(),
        log_level: state.log_level.unwrap_or_default(),
        dispatch_state,
        stop_mode: state.stop_mode,
        in_flight,
    })
}

fn mutation_identity(
    key: &openengine_cluster_protocol::IdempotencyKey,
    fingerprint: &RequestFingerprint,
) -> Result<MutationIdentity, AdmissionStoreError> {
    Ok(MutationIdentity {
        key: IdempotencyId::new(key.as_str())
            .map_err(|_| AdmissionStoreError::SchemaViolation("invalid idempotency key".into()))?,
        fingerprint: decode_digest(fingerprint.as_str())?,
    })
}

fn enforce_generation(
    expected: Option<Generation>,
    actual: Option<LedgerGeneration>,
) -> Result<(), AdmissionStoreError> {
    let actual = actual.map(protocol_generation).transpose()?;
    if expected.is_some() && expected != actual {
        Err(AdmissionStoreError::GenerationConflict { current: actual })
    } else {
        Ok(())
    }
}

fn protocol_generation(value: LedgerGeneration) -> Result<Generation, AdmissionStoreError> {
    Generation::new(value.get())
        .map_err(|_| AdmissionStoreError::Internal("generation exceeds protocol bound".into()))
}

fn protocol_phase(phase: LedgerPhase) -> Phase {
    match phase {
        LedgerPhase::Empty => Phase::Empty,
        LedgerPhase::Running => Phase::Running,
        LedgerPhase::Terminal => Phase::Finished,
    }
}

fn cursor(position: Position) -> Cursor {
    Cursor::new(format!("ledger-{}", position.get()))
}

fn fingerprint_from_bytes(
    fingerprint: [u8; 32],
) -> Result<RequestFingerprint, AdmissionStoreError> {
    let encoded = format!("{}", HexDigest(fingerprint));
    serde_json::from_value(serde_json::Value::String(encoded))
        .map_err(|_| AdmissionStoreError::Internal("corrupt fingerprint".into()))
}

fn decode_digest(value: &str) -> Result<[u8; 32], AdmissionStoreError> {
    if value.len() != 64 {
        return Err(AdmissionStoreError::Internal("invalid fingerprint".into()));
    }
    let mut decoded = [0u8; 32];
    for (index, byte) in decoded.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| AdmissionStoreError::Internal("invalid fingerprint".into()))?;
    }
    Ok(decoded)
}

struct HexDigest([u8; 32]);

impl std::fmt::Display for HexDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

fn map_error(error: LedgerError) -> AdmissionStoreError {
    match error {
        LedgerError::IdempotencyConflict => AdmissionStoreError::IdempotencyReuse,
        LedgerError::IllegalTransition => AdmissionStoreError::InvalidPhase {
            current: Phase::Finished,
        },
        LedgerError::ConcurrentMutation => {
            AdmissionStoreError::Internal("concurrent ledger mutation".into())
        }
        LedgerError::FenceRejected => AdmissionStoreError::Internal("owner fence rejected".into()),
        LedgerError::InvalidMutation => {
            AdmissionStoreError::SchemaViolation("invalid ledger mutation".into())
        }
        LedgerError::Cancelled => AdmissionStoreError::Cancelled,
        LedgerError::AlreadyExists | LedgerError::NotFound | LedgerError::Fault(_) => {
            AdmissionStoreError::Internal("native ledger operation failed".into())
        }
    }
}

#[allow(dead_code)]
fn node_identity(value: &str) -> Result<NodeInstanceId, AdmissionStoreError> {
    NodeInstanceId::new(value)
        .map_err(|_| AdmissionStoreError::SchemaViolation("invalid node identity".into()))
}

#[allow(dead_code)]
fn cancellation_observer(signal: &CancellationSignal) -> CancellationObserver {
    signal.observer()
}
