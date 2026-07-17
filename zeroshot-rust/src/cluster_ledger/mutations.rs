use serde::{Deserialize, Serialize};

use crate::fault::{EngineFault, FaultContext};

use super::record::{
    CanonicalDigest, EffectId, ExecutionId, GenerationId, NodeInstanceId, RecordPayload,
    RunSequence,
};
use super::store::{IdempotencyId, Position};
use super::{ClusterLedger, LedgerError, LedgerErrorKind, MutationIdentity};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmissionRequest {
    pub graph_digest: CanonicalDigest,
    pub input_digest: CanonicalDigest,
    pub policy_digest: CanonicalDigest,
    pub catalog_digest: CanonicalDigest,
    pub profile_digest: CanonicalDigest,
    pub absolute_deadline_ms: u64,
    pub verified_input: Vec<u8>,
    pub canonical_graph: Vec<u8>,
    pub canonical_compiled_ir: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AdmissionAllocation {
    pub generation: GenerationId,
    pub run: RunSequence,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DispatchAllocation {
    pub run: RunSequence,
    pub node_instance: NodeInstanceId,
    pub execution: ExecutionId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SettlementResult {
    pub execution: ExecutionId,
    pub accepted: bool,
    pub authoritative_digest: CanonicalDigest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EffectIntentResult {
    pub effect: EffectId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SafeFaultResult {
    pub execution: Option<ExecutionId>,
    pub terminal: bool,
    pub outcome_digest: CanonicalDigest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommitResult<T> {
    pub value: T,
    pub position: Position,
    pub replayed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SafeFaultConsequence {
    Settle {
        execution: ExecutionId,
        outcome_digest: CanonicalDigest,
    },
    Terminal {
        outcome_digest: CanonicalDigest,
    },
}

struct SafeFaultPlan {
    run: RunSequence,
    execution: Option<ExecutionId>,
    consequence_payload: RecordPayload,
    response: SafeFaultResult,
}

fn admission_is_legal(state: &super::ReplayState, if_generation: Option<GenerationId>) -> bool {
    let generation_matches = match (if_generation, state.admission.as_ref()) {
        (None, None) => true,
        (Some(expected), Some(current)) => expected == current.generation,
        _ => false,
    };
    generation_matches
        && state.terminal_outcome.is_none()
        && state.active_dispatches.is_empty()
        && !state
            .effects
            .values()
            .any(|effect| effect.receipt_digest.is_none())
}

fn admission_request_is_canonical(request: &AdmissionRequest) -> bool {
    CanonicalDigest::of(&request.verified_input) == request.input_digest
        && (request.canonical_graph.is_empty()
            || CanonicalDigest::of(&request.canonical_graph) == request.graph_digest)
}

fn admission_payloads(
    generation: GenerationId,
    run: RunSequence,
    request: AdmissionRequest,
) -> Vec<RecordPayload> {
    vec![
        RecordPayload::Admission {
            generation,
            run,
            graph_digest: request.graph_digest,
            input_digest: request.input_digest,
            policy_digest: request.policy_digest,
            catalog_digest: request.catalog_digest,
            profile_digest: request.profile_digest,
            absolute_deadline_ms: request.absolute_deadline_ms,
            canonical_graph: request.canonical_graph,
            canonical_compiled_ir: request.canonical_compiled_ir,
        },
        RecordPayload::VerifiedInput {
            run,
            digest: request.input_digest,
            canonical_bytes: request.verified_input,
        },
    ]
}

fn settlement_payloads(
    run: RunSequence,
    execution: ExecutionId,
    outcome_digest: CanonicalDigest,
    accepted: bool,
    verified_output: Option<Vec<u8>>,
) -> Vec<RecordPayload> {
    let mut payloads = vec![RecordPayload::Settlement {
        run,
        execution,
        outcome_digest,
        accepted,
    }];
    if accepted {
        if let Some(canonical_bytes) = verified_output {
            payloads.push(RecordPayload::VerifiedOutput {
                run,
                execution,
                digest: outcome_digest,
                canonical_bytes,
            });
        }
    }
    payloads
}

impl ClusterLedger {
    pub async fn admit(
        &self,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        request: AdmissionRequest,
    ) -> Result<CommitResult<AdmissionAllocation>, LedgerError> {
        self.admit_cas(key, fingerprint, None, request).await
    }

    pub async fn admit_next(
        &self,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        if_generation: GenerationId,
        request: AdmissionRequest,
    ) -> Result<CommitResult<AdmissionAllocation>, LedgerError> {
        self.admit_cas(key, fingerprint, Some(if_generation), request)
            .await
    }

    async fn admit_cas(
        &self,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        if_generation: Option<GenerationId>,
        request: AdmissionRequest,
    ) -> Result<CommitResult<AdmissionAllocation>, LedgerError> {
        let mut state = self.validated_state(FaultContext::Admission).await?;
        if let Some(receipt) =
            self.existing_receipt(FaultContext::Admission, &state, &key, "admit", fingerprint)?
        {
            return Ok(receipt);
        }
        if !admission_is_legal(&state, if_generation) {
            return Err(
                self.domain_error(FaultContext::Admission, LedgerErrorKind::InvalidLifecycle)
            );
        }
        if !admission_request_is_canonical(&request) {
            return Err(self.domain_error(FaultContext::Admission, LedgerErrorKind::Encoding));
        }
        let generation = state.identities.allocate_generation().map_err(|_| {
            self.domain_error(FaultContext::Admission, LedgerErrorKind::BoundViolation)
        })?;
        let run = state.identities.allocate_run().map_err(|_| {
            self.domain_error(FaultContext::Admission, LedgerErrorKind::BoundViolation)
        })?;
        let response = AdmissionAllocation { generation, run };
        let payloads = admission_payloads(generation, run, request);
        self.commit(
            FaultContext::Admission,
            &state,
            MutationIdentity::new(key, "admit", fingerprint),
            payloads,
            &response,
        )
        .await
    }

    pub async fn dispatch(
        &self,
        key: IdempotencyId,
        fingerprint: [u8; 32],
    ) -> Result<CommitResult<DispatchAllocation>, LedgerError> {
        let mut state = self.validated_state(FaultContext::Execution).await?;
        if let Some(receipt) = self.existing_receipt(
            FaultContext::Execution,
            &state,
            &key,
            "dispatch",
            fingerprint,
        )? {
            return Ok(receipt);
        }
        let admission = state.admission.as_ref().ok_or_else(|| {
            self.domain_error(FaultContext::Execution, LedgerErrorKind::InvalidLifecycle)
        })?;
        if state.terminal_outcome.is_some() {
            return Err(
                self.domain_error(FaultContext::Execution, LedgerErrorKind::InvalidLifecycle)
            );
        }
        let run = admission.run;
        let node_instance = state.identities.allocate_node_instance().map_err(|_| {
            self.domain_error(FaultContext::Execution, LedgerErrorKind::BoundViolation)
        })?;
        let execution = state.identities.allocate_execution().map_err(|_| {
            self.domain_error(FaultContext::Execution, LedgerErrorKind::BoundViolation)
        })?;
        let response = DispatchAllocation {
            run,
            node_instance,
            execution,
        };
        self.commit(
            FaultContext::Execution,
            &state,
            MutationIdentity::new(key, "dispatch", fingerprint),
            vec![RecordPayload::Dispatch {
                run,
                node_instance,
                execution,
            }],
            &response,
        )
        .await
    }

    fn require_settleable(&self, state: &super::ReplayState) -> Result<(), LedgerError> {
        if state.admission.is_none() || state.terminal_outcome.is_some() {
            return Err(
                self.domain_error(FaultContext::Settlement, LedgerErrorKind::InvalidLifecycle)
            );
        }
        Ok(())
    }

    fn settlement_authority(
        &self,
        state: &super::ReplayState,
        execution: ExecutionId,
        outcome_digest: CanonicalDigest,
    ) -> Result<(RunSequence, bool, CanonicalDigest), LedgerError> {
        if let Some(dispatch) = state.active_dispatches.get(&execution) {
            return Ok((dispatch.run, true, outcome_digest));
        }
        let run = *state.settlement_runs.get(&execution).ok_or_else(|| {
            self.domain_error(FaultContext::Settlement, LedgerErrorKind::InvalidSettlement)
        })?;
        let authoritative_digest = *state.settlements.get(&execution).ok_or_else(|| {
            self.domain_error(FaultContext::Settlement, LedgerErrorKind::InvalidSettlement)
        })?;
        Ok((run, false, authoritative_digest))
    }

    pub async fn settle(
        &self,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        execution: ExecutionId,
        outcome_digest: CanonicalDigest,
        verified_output: Option<Vec<u8>>,
    ) -> Result<CommitResult<SettlementResult>, LedgerError> {
        let state = self.validated_state(FaultContext::Settlement).await?;
        if let Some(receipt) = self.existing_receipt(
            FaultContext::Settlement,
            &state,
            &key,
            "settle",
            fingerprint,
        )? {
            return Ok(receipt);
        }
        self.require_settleable(&state)?;
        if verified_output
            .as_ref()
            .is_some_and(|bytes| CanonicalDigest::of(bytes) != outcome_digest)
        {
            return Err(self.domain_error(FaultContext::Settlement, LedgerErrorKind::Encoding));
        }
        let (run, accepted, authoritative_digest) =
            self.settlement_authority(&state, execution, outcome_digest)?;
        let response = SettlementResult {
            execution,
            accepted,
            authoritative_digest,
        };
        let payloads =
            settlement_payloads(run, execution, outcome_digest, accepted, verified_output);
        self.commit(
            FaultContext::Settlement,
            &state,
            MutationIdentity::new(key, "settle", fingerprint),
            payloads,
            &response,
        )
        .await
    }

    fn safe_fault_plan(
        &self,
        state: &super::ReplayState,
        consequence: SafeFaultConsequence,
    ) -> Result<SafeFaultPlan, LedgerError> {
        let run = self.safe_fault_run(state, consequence)?;
        let (execution, terminal, outcome_digest, consequence_payload) = match consequence {
            SafeFaultConsequence::Settle {
                execution,
                outcome_digest,
            } if state.active_dispatches.contains_key(&execution) => (
                Some(execution),
                false,
                outcome_digest,
                RecordPayload::Settlement {
                    run,
                    execution,
                    outcome_digest,
                    accepted: true,
                },
            ),
            SafeFaultConsequence::Settle { .. } => {
                return Err(
                    self.domain_error(FaultContext::Settlement, LedgerErrorKind::InvalidSettlement)
                );
            }
            SafeFaultConsequence::Terminal { outcome_digest } => (
                None,
                true,
                outcome_digest,
                RecordPayload::Terminal {
                    run,
                    outcome_digest,
                },
            ),
        };
        Ok(SafeFaultPlan {
            run,
            execution,
            consequence_payload,
            response: SafeFaultResult {
                execution,
                terminal,
                outcome_digest,
            },
        })
    }

    fn safe_fault_run(
        &self,
        state: &super::ReplayState,
        consequence: SafeFaultConsequence,
    ) -> Result<RunSequence, LedgerError> {
        let run = state
            .admission
            .as_ref()
            .filter(|_| state.terminal_outcome.is_none())
            .ok_or_else(|| {
                self.domain_error(FaultContext::Settlement, LedgerErrorKind::InvalidLifecycle)
            })?
            .run;
        if matches!(consequence, SafeFaultConsequence::Terminal { .. })
            && state
                .effects
                .values()
                .any(|effect| effect.receipt_digest.is_none())
        {
            return Err(
                self.domain_error(FaultContext::Settlement, LedgerErrorKind::InvalidLifecycle)
            );
        }
        Ok(run)
    }

    pub async fn record_safe_fault(
        &self,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        fault: &EngineFault,
        consequence: SafeFaultConsequence,
    ) -> Result<CommitResult<SafeFaultResult>, LedgerError> {
        let state = self.validated_state(FaultContext::Settlement).await?;
        if let Some(receipt) = self.existing_receipt(
            FaultContext::Settlement,
            &state,
            &key,
            "safe_fault",
            fingerprint,
        )? {
            return Ok(receipt);
        }
        let plan = self.safe_fault_plan(&state, consequence)?;
        let encoded_fault = fault
            .encode_json()
            .map_err(|_| self.domain_error(FaultContext::Settlement, LedgerErrorKind::Encoding))?;
        self.commit(
            FaultContext::Settlement,
            &state,
            MutationIdentity::new(key, "safe_fault", fingerprint),
            vec![
                RecordPayload::SafeFault {
                    run: plan.run,
                    execution: plan.execution,
                    encoded_fault,
                },
                plan.consequence_payload,
            ],
            &plan.response,
        )
        .await
    }

    pub async fn record_effect_intent(
        &self,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        request_digest: CanonicalDigest,
    ) -> Result<CommitResult<EffectIntentResult>, LedgerError> {
        let mut state = self.validated_state(FaultContext::Execution).await?;
        if let Some(receipt) = self.existing_receipt(
            FaultContext::Execution,
            &state,
            &key,
            "effect_intent",
            fingerprint,
        )? {
            return Ok(receipt);
        }
        let run = state
            .admission
            .as_ref()
            .ok_or_else(|| {
                self.domain_error(FaultContext::Execution, LedgerErrorKind::InvalidLifecycle)
            })?
            .run;
        if state.terminal_outcome.is_some() {
            return Err(
                self.domain_error(FaultContext::Execution, LedgerErrorKind::InvalidLifecycle)
            );
        }
        let effect = state.identities.allocate_effect().map_err(|_| {
            self.domain_error(FaultContext::Execution, LedgerErrorKind::BoundViolation)
        })?;
        let response = EffectIntentResult { effect };
        self.commit(
            FaultContext::Execution,
            &state,
            MutationIdentity::new(key, "effect_intent", fingerprint),
            vec![RecordPayload::EffectIntent {
                run,
                effect,
                request_digest,
            }],
            &response,
        )
        .await
    }

    pub async fn reconcile_effect(
        &self,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        effect: EffectId,
        receipt_digest: CanonicalDigest,
    ) -> Result<CommitResult<EffectIntentResult>, LedgerError> {
        let state = self.validated_state(FaultContext::Settlement).await?;
        if let Some(receipt) = self.existing_receipt(
            FaultContext::Settlement,
            &state,
            &key,
            "effect_receipt",
            fingerprint,
        )? {
            return Ok(receipt);
        }
        let run = state
            .admission
            .as_ref()
            .ok_or_else(|| {
                self.domain_error(FaultContext::Settlement, LedgerErrorKind::InvalidLifecycle)
            })?
            .run;
        if state.terminal_outcome.is_some()
            || state
                .effects
                .get(&effect)
                .is_none_or(|intent| intent.receipt_digest.is_some())
        {
            return Err(
                self.domain_error(FaultContext::Settlement, LedgerErrorKind::InvalidLifecycle)
            );
        }
        let response = EffectIntentResult { effect };
        self.commit(
            FaultContext::Settlement,
            &state,
            MutationIdentity::new(key, "effect_receipt", fingerprint),
            vec![RecordPayload::EffectReceipt {
                run,
                effect,
                receipt_digest,
            }],
            &response,
        )
        .await
    }

    pub async fn terminalize(
        &self,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        outcome_digest: CanonicalDigest,
    ) -> Result<CommitResult<CanonicalDigest>, LedgerError> {
        let state = self.validated_state(FaultContext::Settlement).await?;
        if let Some(receipt) = self.existing_receipt(
            FaultContext::Settlement,
            &state,
            &key,
            "terminal",
            fingerprint,
        )? {
            return Ok(receipt);
        }
        let run = state
            .admission
            .as_ref()
            .ok_or_else(|| {
                self.domain_error(FaultContext::Settlement, LedgerErrorKind::InvalidLifecycle)
            })?
            .run;
        if state.terminal_outcome.is_some()
            || state
                .effects
                .values()
                .any(|effect| effect.receipt_digest.is_none())
        {
            return Err(
                self.domain_error(FaultContext::Settlement, LedgerErrorKind::InvalidLifecycle)
            );
        }
        self.commit(
            FaultContext::Settlement,
            &state,
            MutationIdentity::new(key, "terminal", fingerprint),
            vec![RecordPayload::Terminal {
                run,
                outcome_digest,
            }],
            &outcome_digest,
        )
        .await
    }

    pub async fn record_cleanup_receipt(
        &self,
        key: IdempotencyId,
        fingerprint: [u8; 32],
        cleanup_digest: CanonicalDigest,
    ) -> Result<CommitResult<CanonicalDigest>, LedgerError> {
        let state = self.validated_state(FaultContext::Cleanup).await?;
        if let Some(receipt) = self.existing_receipt(
            FaultContext::Cleanup,
            &state,
            &key,
            "cleanup_receipt",
            fingerprint,
        )? {
            return Ok(receipt);
        }
        if state.terminal_outcome.is_none() {
            return Err(self.domain_error(FaultContext::Cleanup, LedgerErrorKind::TerminalRequired));
        }
        self.commit(
            FaultContext::Cleanup,
            &state,
            MutationIdentity::new(key, "cleanup_receipt", fingerprint),
            vec![RecordPayload::CleanupReceipt { cleanup_digest }],
            &cleanup_digest,
        )
        .await
    }
}
