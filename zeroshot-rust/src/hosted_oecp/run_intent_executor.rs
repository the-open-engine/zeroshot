use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    legacy_ship_request_payload_type, legacy_ship_result_payload_type, ApplyParams, Generation,
    GraphSpec, IdempotencyKey, LegacyShipRequest, LegacyShipResult, LegacyShipStatus,
    WorkerOutcome, INTERNAL_ERROR_CODE,
};
use openengine_cluster_server::BackendError;
use serde_json::{json, Value};
use tokio::sync::{watch, Mutex};

use super::{backend::HostedBackend, credentials::CredentialBundle, run_intent::MAX_RUN_INTENT_BYTES};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RunIntentIdentity {
    intent_id: String,
    digest: String,
}

impl RunIntentIdentity {
    pub(super) fn new(intent_id: String, digest: String) -> Self {
        Self { intent_id, digest }
    }

    pub(super) fn intent_id(&self) -> &str {
        &self.intent_id
    }

    pub(super) fn digest(&self) -> &str {
        &self.digest
    }
}

pub(super) struct RunIntentSubmission {
    identity: RunIntentIdentity,
    credentials: CredentialBundle,
    request: LegacyShipRequest,
}

impl RunIntentSubmission {
    pub(super) fn new(
        identity: RunIntentIdentity,
        credentials: CredentialBundle,
        request: LegacyShipRequest,
    ) -> Self {
        Self {
            identity,
            credentials,
            request,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum RunIntentStatus {
    Running,
    Succeeded(Value),
    Failed(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum RunIntentLookup {
    Found(RunIntentStatus),
    NotFound,
    Conflict,
}

#[async_trait]
pub(super) trait RunIntentExecutor: Send + Sync {
    async fn submit(&self, submission: RunIntentSubmission) -> Result<RunIntentStatus, ()>;

    async fn lookup(&self, identity: &RunIntentIdentity) -> RunIntentLookup;
}

#[derive(Clone, Debug, PartialEq)]
struct RunIntentRecord {
    identity: RunIntentIdentity,
    status: RunIntentStatus,
}

#[derive(Clone, Debug, PartialEq)]
enum RunIntentReservation {
    Reserved,
    Existing(RunIntentStatus),
    Conflict,
}

#[derive(Clone)]
pub(super) struct HostedRunIntentExecutor {
    backend: Arc<HostedBackend>,
    record: Arc<Mutex<Option<RunIntentRecord>>>,
}

impl HostedRunIntentExecutor {
    pub(super) fn new(backend: Arc<HostedBackend>) -> Self {
        Self {
            backend,
            record: Arc::new(Mutex::new(None)),
        }
    }

    async fn reserve(&self, identity: RunIntentIdentity) -> RunIntentReservation {
        let mut record = self.record.lock().await;
        if let Some(existing) = record.as_ref() {
            return if existing.identity == identity {
                RunIntentReservation::Existing(existing.status.clone())
            } else {
                RunIntentReservation::Conflict
            };
        }
        if self.backend.reserve_internal_admission().await.is_err() {
            return RunIntentReservation::Conflict;
        }
        *record = Some(RunIntentRecord {
            identity,
            status: RunIntentStatus::Running,
        });
        RunIntentReservation::Reserved
    }

    async fn execute(&self, submission: RunIntentSubmission) {
        let mut completion = self.backend.subscribe_completion();
        let params = run_intent_apply_params(submission.identity.intent_id(), submission.request);
        let started = match params {
            Ok(params) => {
                self.backend
                    .begin_reserved_run(params, &submission.credentials)
                    .await
            }
            Err(error) => Err(error),
        };
        if started.is_err() {
            self.backend.fail_reserved_admission().await;
            self.finish(
                &submission.identity,
                RunIntentStatus::Failed("worker_start_failed"),
            )
            .await;
            return;
        }
        let status = wait_for_completion(&mut completion)
            .await
            .map_or(RunIntentStatus::Failed("worker_exited"), |outcome| {
                run_intent_status(&outcome)
            });
        self.finish(&submission.identity, status).await;
    }

    async fn finish(&self, identity: &RunIntentIdentity, status: RunIntentStatus) {
        let mut record = self.record.lock().await;
        let Some(record) = record.as_mut() else {
            return;
        };
        if &record.identity == identity && matches!(record.status, RunIntentStatus::Running) {
            record.status = status;
        }
    }
}

#[async_trait]
impl RunIntentExecutor for HostedRunIntentExecutor {
    async fn submit(&self, submission: RunIntentSubmission) -> Result<RunIntentStatus, ()> {
        match self.reserve(submission.identity.clone()).await {
            RunIntentReservation::Existing(status) => Ok(status),
            RunIntentReservation::Conflict => Err(()),
            RunIntentReservation::Reserved => {
                let executor = self.clone();
                tokio::spawn(async move {
                    executor.execute(submission).await;
                });
                Ok(RunIntentStatus::Running)
            }
        }
    }

    async fn lookup(&self, identity: &RunIntentIdentity) -> RunIntentLookup {
        let record = self.record.lock().await;
        let Some(record) = record.as_ref() else {
            return RunIntentLookup::NotFound;
        };
        if record.identity.intent_id != identity.intent_id {
            return RunIntentLookup::NotFound;
        }
        if record.identity.digest != identity.digest {
            return RunIntentLookup::Conflict;
        }
        RunIntentLookup::Found(record.status.clone())
    }
}

async fn wait_for_completion(
    completion: &mut watch::Receiver<Option<WorkerOutcome>>,
) -> Option<WorkerOutcome> {
    loop {
        if let Some(outcome) = completion.borrow().clone() {
            return Some(outcome);
        }
        if completion.changed().await.is_err() {
            return None;
        }
    }
}

fn run_intent_apply_params(
    intent_id: &str,
    request: LegacyShipRequest,
) -> Result<ApplyParams, BackendError> {
    let input = serde_json::to_value(request)
        .map_err(|error| BackendError::new(INTERNAL_ERROR_CODE, error.to_string()))?;
    Ok(ApplyParams {
        graph: intent_graph()?,
        input: Some(input),
        dry_run: false,
        if_generation: Some(Generation::new(0).expect("zero is a safe generation")),
        idempotency_key: Some(
            IdempotencyKey::new(format!("run-intent:{intent_id}"))
                .map_err(|error| BackendError::new(INTERNAL_ERROR_CODE, error))?,
        ),
    })
}

fn intent_graph() -> Result<GraphSpec, BackendError> {
    serde_json::from_value(json!({
        "profile": "openengine.graph.single-worker/v1",
        "initialInput": legacy_ship_request_payload_type(),
        "policy": { "policy": "policy.strict@1", "default": "deny" },
        "root": {
            "kind": "step",
            "name": "zeroshot",
            "worker": "legacy.zeroshot.ship@1",
            "input": legacy_ship_request_payload_type(),
            "output": legacy_ship_result_payload_type(),
            "inputBindings": [],
            "writeBindings": [],
            "timeoutMs": 3_600_000,
            "attempts": 1
        }
    }))
    .map_err(|error| BackendError::new(INTERNAL_ERROR_CODE, error.to_string()))
}

fn run_intent_status(outcome: &WorkerOutcome) -> RunIntentStatus {
    match outcome {
        WorkerOutcome::Verified { output, .. } => {
            let Ok(result) = serde_json::from_value::<LegacyShipResult>(output.clone()) else {
                return RunIntentStatus::Failed("malformed_result");
            };
            if result.status == LegacyShipStatus::Failed {
                return RunIntentStatus::Failed("worker_failed");
            }
            let response = json!({ "state": "succeeded", "result": output });
            if serde_json::to_vec(&response).is_ok_and(|bytes| bytes.len() <= MAX_RUN_INTENT_BYTES)
            {
                RunIntentStatus::Succeeded(output.clone())
            } else {
                RunIntentStatus::Failed("result_too_large")
            }
        }
        WorkerOutcome::Verifier { .. } => RunIntentStatus::Failed("verification_failed"),
        WorkerOutcome::Error { code, .. } => RunIntentStatus::Failed(code.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use openengine_cluster_protocol::{GraphNode, GraphProfile, LegacyShipRequest};
    use serde_json::{json, Value};

    use super::{intent_graph, run_intent_apply_params};

    #[test]
    fn run_intent_adapter_builds_the_canonical_single_worker_graph() {
        let graph = intent_graph().expect("intent graph is valid");
        assert_eq!(graph.profile, GraphProfile::SingleWorker);
        let GraphNode::Step(root) = &graph.root else {
            panic!("intent graph root must be a step");
        };
        assert_eq!(root.worker.as_str(), "legacy.zeroshot.ship@1");

        let fixture: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/hosted/run-intent-v1.json"
        ))
        .expect("golden fixture is JSON");
        let request: LegacyShipRequest =
            serde_json::from_value(fixture["envelope"]["request"].clone())
                .expect("golden request is valid");
        let canonical_request =
            serde_json::to_value(&request).expect("golden request serializes canonically");
        let params = run_intent_apply_params("019f7437-8701-71e3-a056-2ba05c37609c", request)
            .expect("golden request maps into apply params");
        assert_eq!(params.graph, graph);
        assert_eq!(
            params.idempotency_key.expect("idempotency key").as_str(),
            "run-intent:019f7437-8701-71e3-a056-2ba05c37609c"
        );
        assert_eq!(params.input.expect("request input"), canonical_request);
        assert_eq!(json!(params.dry_run), json!(false));
    }

    #[test]
    fn hosted_backend_contains_no_run_intent_control_plane_state() {
        let backend = include_str!("backend.rs");
        assert!(!backend.contains("RunIntent"));
        assert!(!backend.contains("run_intent"));
    }
}
