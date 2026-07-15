//! Backend-neutral admission orchestration and durable-store ports.

use std::sync::Arc;

mod ports;
pub use ports::*;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    admission_fingerprint, diff_compiled_graphs, ApplyParams, ApplyResult, CompiledGraphIr,
    Generation, GetParams, GetResult, GraphSpec, IdempotencyKey, InitializeParams,
    InitializeResult, Phase, PlanParams, PlanResult, RequestFingerprint, ServerCapabilities,
    CANCELLED, GENERATION_CONFLICT, GRAPH_INVALID, IDEMPOTENCY_REUSE, INTERNAL_ERROR_CODE,
    INVALID_PHASE, SCHEMA_VIOLATION,
};
use serde_json::{json, Value};

use crate::{BackendError, ClusterBackend, ConnectionContext};

pub struct AdmissionCoordinator<V, S> {
    verifier: Arc<V>,
    store: Arc<S>,
}

struct PreparedCommit {
    params: ApplyParams,
    fingerprint: RequestFingerprint,
    verified: VerifiedGraph,
    snapshot: AdmissionSnapshot,
}

impl<V, S> Clone for AdmissionCoordinator<V, S> {
    fn clone(&self) -> Self {
        Self {
            verifier: Arc::clone(&self.verifier),
            store: Arc::clone(&self.store),
        }
    }
}

impl<V, S> AdmissionCoordinator<V, S> {
    #[must_use]
    pub fn new(verifier: V, store: S) -> Self {
        Self {
            verifier: Arc::new(verifier),
            store: Arc::new(store),
        }
    }

    #[must_use]
    pub fn from_shared(verifier: Arc<V>, store: Arc<S>) -> Self {
        Self { verifier, store }
    }

    #[must_use]
    pub fn store(&self) -> &Arc<S> {
        &self.store
    }

    #[must_use]
    pub fn verifier(&self) -> &Arc<V> {
        &self.verifier
    }
}

impl<V, S> AdmissionCoordinator<V, S>
where
    V: GraphVerifier,
    S: AdmissionStore,
{
    async fn read_valid_snapshot(&self) -> Result<AdmissionSnapshot, BackendError> {
        let snapshot = self
            .store
            .read_snapshot()
            .await
            .map_err(store_error_to_backend)?;
        validate_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    async fn verify_for_apply(&self, graph: &GraphSpec) -> Result<VerifiedGraph, BackendError> {
        match self.verifier.verify(graph).await {
            Ok(verified) => {
                diff_compiled_graphs(None, &verified.compiled_ir).map_err(|error| {
                    BackendError::application(
                        GRAPH_INVALID,
                        "Graph verifier returned invalid compiled IR",
                        Some(json!({ "reason": error.to_string() })),
                    )
                })?;
                Ok(verified)
            }
            Err(VerificationError::Rejected { diagnostics }) => Err(BackendError::application(
                GRAPH_INVALID,
                "Graph verification failed",
                Some(json!({ "diagnostics": diagnostics })),
            )),
            Err(VerificationError::Internal(message)) => {
                Err(BackendError::new(INTERNAL_ERROR_CODE, message))
            }
        }
    }

    async fn replay_if_known(
        &self,
        key: &IdempotencyKey,
        fingerprint: &RequestFingerprint,
    ) -> Result<Option<ApplyResult>, BackendError> {
        let record = self
            .store
            .lookup_idempotency(key)
            .await
            .map_err(store_error_to_backend)?;
        match record {
            Some(record) if record.fingerprint == *fingerprint => {
                let mut receipt = record.receipt;
                receipt.deduped = true;
                Ok(Some(receipt))
            }
            Some(_) => Err(BackendError::application(
                IDEMPOTENCY_REUSE,
                "Idempotency key was reused with different parameters",
                None,
            )),
            None => Ok(None),
        }
    }

    async fn replay_apply(
        &self,
        params: &ApplyParams,
        fingerprint: &RequestFingerprint,
    ) -> Result<Option<ApplyResult>, BackendError> {
        match &params.idempotency_key {
            Some(key) => self.replay_if_known(key, fingerprint).await,
            None => Ok(None),
        }
    }

    async fn commit_verified(
        &self,
        context: &ConnectionContext,
        prepared: PreparedCommit,
    ) -> Result<ApplyResult, BackendError> {
        let PreparedCommit {
            params,
            fingerprint,
            verified,
            snapshot,
        } = prepared;
        precheck_input(
            snapshot.control.compiled_ir.as_ref(),
            &verified.compiled_ir,
            &params.graph,
            params.input.as_ref(),
        )?;
        if context.cancellation.is_cancelled() {
            return Err(cancelled_error());
        }
        let proposal = CommitProposal {
            graph: params.graph,
            compiled_ir: verified.compiled_ir,
            input: params.input,
            if_generation: params.if_generation,
            idempotency_key: params
                .idempotency_key
                .expect("committed apply mode requires an idempotency key"),
            fingerprint,
        };
        self.store
            .commit(proposal, &context.cancellation)
            .await
            .map_err(store_error_to_backend)
    }
}

#[async_trait]
impl<V, S> ClusterBackend for AdmissionCoordinator<V, S>
where
    V: GraphVerifier,
    S: AdmissionStore,
{
    async fn initialize(
        &self,
        _context: &ConnectionContext,
        _params: InitializeParams,
    ) -> Result<InitializeResult, BackendError> {
        let snapshot = self.read_valid_snapshot().await?;
        Ok(InitializeResult::new(
            ServerCapabilities::default(),
            snapshot.control.status(),
        ))
    }

    async fn plan(
        &self,
        _context: &ConnectionContext,
        params: PlanParams,
    ) -> Result<PlanResult, BackendError> {
        match self.verifier.verify(&params.graph).await {
            Ok(verified) => {
                diff_compiled_graphs(None, &verified.compiled_ir).map_err(|error| {
                    BackendError::application(
                        GRAPH_INVALID,
                        "Graph verifier returned invalid compiled IR",
                        Some(json!({ "reason": error.to_string() })),
                    )
                })?;
                Ok(PlanResult {
                    ok: true,
                    diagnostics: verified.diagnostics,
                    bounds: Some(verified.compiled_ir.bounds),
                })
            }
            Err(VerificationError::Rejected { diagnostics }) => Ok(PlanResult {
                ok: false,
                diagnostics,
                bounds: None,
            }),
            Err(VerificationError::Internal(message)) => {
                Err(BackendError::new(INTERNAL_ERROR_CODE, message))
            }
        }
    }

    async fn apply(
        &self,
        context: &ConnectionContext,
        params: ApplyParams,
    ) -> Result<ApplyResult, BackendError> {
        validate_apply_mode(&params)?;
        let fingerprint = apply_fingerprint(&params)?;

        if let Some(receipt) = self.replay_apply(&params, &fingerprint).await? {
            return Ok(receipt);
        }

        let verified = self.verify_for_apply(&params.graph).await?;
        let snapshot = self.read_valid_snapshot().await?;
        precheck_generation(params.if_generation, snapshot.control.generation)?;

        let diff =
            diff_compiled_graphs(snapshot.control.compiled_ir.as_ref(), &verified.compiled_ir)
                .map_err(|error| {
                    BackendError::application(
                        GRAPH_INVALID,
                        "Graph verifier returned invalid compiled IR",
                        Some(json!({ "reason": error.to_string() })),
                    )
                })?;

        if params.dry_run {
            return Ok(ApplyResult {
                generation: snapshot.control.generation,
                run_id: snapshot.control.run_id,
                phase: snapshot.control.phase,
                deduped: false,
                diff: Some(diff),
            });
        }

        self.commit_verified(
            context,
            PreparedCommit {
                params,
                fingerprint,
                verified,
                snapshot,
            },
        )
        .await
    }

    async fn get(
        &self,
        _context: &ConnectionContext,
        params: GetParams,
    ) -> Result<GetResult, BackendError> {
        let snapshot = self.read_valid_snapshot().await?;
        if let Some(requested) = params.at_cursor {
            if snapshot.control.cursor.as_ref() != Some(&requested) {
                return Err(BackendError::application(
                    INVALID_PHASE,
                    "Requested cursor is not available",
                    Some(json!({ "currentCursor": snapshot.control.cursor })),
                ));
            }
        }
        let status = snapshot.control.status();
        Ok(GetResult {
            spec: snapshot.control.spec,
            status,
            at_cursor: snapshot.control.cursor,
        })
    }
}

fn apply_fingerprint(params: &ApplyParams) -> Result<RequestFingerprint, BackendError> {
    let value = serde_json::to_value(params)
        .map_err(|error| BackendError::new(INTERNAL_ERROR_CODE, error.to_string()))?;
    let Value::Object(mut parameters) = value else {
        return Err(BackendError::new(
            INTERNAL_ERROR_CODE,
            "serialized apply parameters were not an object",
        ));
    };
    parameters.remove("idempotencyKey");
    admission_fingerprint("apply", &Value::Object(parameters))
        .map_err(|error| BackendError::new(INTERNAL_ERROR_CODE, error.to_string()))
}

fn validate_snapshot(snapshot: &AdmissionSnapshot) -> Result<(), BackendError> {
    let is_empty = snapshot_is_empty(snapshot);
    let is_committed = snapshot_is_committed(snapshot);
    let valid = match snapshot.control.phase {
        Phase::Empty => is_empty,
        Phase::Admitting => is_empty || is_committed,
        Phase::Running => is_committed,
    };
    if valid {
        Ok(())
    } else {
        Err(BackendError::new(
            INTERNAL_ERROR_CODE,
            "admission store returned an inconsistent phase snapshot",
        ))
    }
}

fn snapshot_is_empty(snapshot: &AdmissionSnapshot) -> bool {
    snapshot.control.spec.is_none()
        && snapshot.control.compiled_ir.is_none()
        && snapshot.control.generation.is_none()
        && snapshot.control.run_id.is_none()
        && snapshot.control.cursor.is_none()
        && snapshot.seed.is_none()
}

fn snapshot_is_committed(snapshot: &AdmissionSnapshot) -> bool {
    matches!(
        (
            snapshot.control.spec.as_ref(),
            snapshot.control.compiled_ir.as_ref(),
            snapshot.control.generation,
            snapshot.control.run_id.as_ref(),
            snapshot.control.cursor.as_ref(),
            snapshot.seed.as_ref(),
        ),
        (
            Some(spec),
            Some(compiled_ir),
            Some(generation),
            Some(run_id),
            Some(cursor),
            Some(seed),
        ) if generation.get() > 0
            && seed.run_id == *run_id
            && seed.cursor == *cursor
            && spec.initial_input.validate_value(&seed.input).is_ok()
            && compiled_ir.identity().is_ok()
    )
}

fn validate_apply_mode(params: &ApplyParams) -> Result<(), BackendError> {
    if params.dry_run {
        if params.idempotency_key.is_some() {
            return Err(schema_error("dry-run apply must omit idempotencyKey"));
        }
        if params.input.is_some() {
            return Err(schema_error("dry-run apply must omit input"));
        }
    } else if params.idempotency_key.is_none() {
        return Err(schema_error("committed apply requires idempotencyKey"));
    }
    Ok(())
}

fn precheck_generation(
    expected: Option<Generation>,
    current: Option<Generation>,
) -> Result<(), BackendError> {
    let matches = match expected {
        None => true,
        Some(expected) if expected.get() == 0 => current.is_none(),
        Some(expected) => current == Some(expected),
    };
    if matches {
        Ok(())
    } else {
        Err(BackendError::application(
            GENERATION_CONFLICT,
            "Generation precondition failed",
            Some(json!({ "currentGeneration": current })),
        ))
    }
}

fn precheck_input(
    current: Option<&CompiledGraphIr>,
    desired: &CompiledGraphIr,
    graph: &GraphSpec,
    input: Option<&Value>,
) -> Result<(), BackendError> {
    let unchanged = current
        .map(|current| Ok(current.identity()? == desired.identity()?))
        .transpose()
        .map_err(|error: openengine_cluster_protocol::CanonicalError| {
            BackendError::new(INTERNAL_ERROR_CODE, error.to_string())
        })?
        .unwrap_or(false);
    if unchanged {
        if input.is_some() {
            return Err(schema_error(
                "unchanged apply must omit input; use future resubmit semantics to supply a new root input",
            ));
        }
        return Ok(());
    }
    let input = input.ok_or_else(|| schema_error("apply that starts a run requires input"))?;
    graph
        .initial_input
        .validate_value(input)
        .map_err(|error| schema_error(&error.to_string()))
}

fn schema_error(message: &str) -> BackendError {
    BackendError::invalid_params(
        SCHEMA_VIOLATION,
        "Admission parameters violate the schema",
        Some(json!({ "reason": message })),
    )
}

fn cancelled_error() -> BackendError {
    BackendError::application(CANCELLED, "Admission cancelled before commit", None)
}

fn store_error_to_backend(error: StoreError) -> BackendError {
    match error {
        StoreError::Internal(message) => BackendError::new(INTERNAL_ERROR_CODE, message),
        StoreError::IdempotencyReuse => BackendError::application(
            IDEMPOTENCY_REUSE,
            "Idempotency key was reused with different parameters",
            None,
        ),
        StoreError::GenerationConflict { current } => BackendError::application(
            GENERATION_CONFLICT,
            "Generation precondition failed",
            Some(json!({ "currentGeneration": current })),
        ),
        StoreError::InvalidPhase { current } => BackendError::application(
            INVALID_PHASE,
            "Cluster phase does not admit apply",
            Some(json!({ "currentPhase": current })),
        ),
        StoreError::SchemaViolation(message) => schema_error(&message),
        StoreError::Cancelled => cancelled_error(),
    }
}
