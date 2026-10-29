//! Stateless TypeSafe-compatible decisions on existing verifier nodes.
use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    EnumLabel, EnvironmentVariableName, ModelId, WorkerErrorCode, WorkerOutcome,
};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use crate::native_v2_contract::{NodeInvocation, NodeRuntimeBinding, parse_token_usage_delta};
use crate::native_v2_runner::{
    DriverControl, DriverInvocation, NodeDriver, NodeRunnerError, NodeSession, ResolvedEnvironment,
    LiveOutput, LiveOutputStream, SessionFactory,
};

pub(crate) mod contract;
use contract::{API_KEY, ENDPOINT, DecisionContract, validate_contract};

const MAX_DECISION_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const DISTRIBUTION_TOLERANCE: f64 = 0.000_001;

fn failure(message: &str) -> NodeRunnerError {
    NodeRunnerError::DriverDetail(message.to_owned())
}

struct DecisionFailure {
    code: WorkerErrorCode,
    message: String,
}

impl DecisionFailure {
    fn new(code: WorkerErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn transport(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::new(WorkerErrorCode::Timeout, "decision HTTP request timed out")
        } else {
            Self::new(WorkerErrorCode::Crash, "decision HTTP transport failed")
        }
    }
}

impl From<NodeRunnerError> for DecisionFailure {
    fn from(error: NodeRunnerError) -> Self {
        Self::new(WorkerErrorCode::Crash, error.to_string())
    }
}

pub(crate) struct DecisionAdapter {
    client: reqwest::Client,
}

impl DecisionAdapter {
    pub(crate) fn new() -> Result<Self, NodeRunnerError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| failure("could not initialize decision HTTP client"))?;
        Ok(Self { client })
    }

    async fn evaluate(
        &self,
        invocation: &DriverInvocation,
        usage: &mut Option<crate::native_v2_contract::TokenUsageDelta>,
    ) -> Result<WorkerOutcome, DecisionFailure> {
        let contract = validate_contract(&invocation.response, &invocation.node.binding)?;
        let body = request_body(invocation, &contract)?;
        let (endpoint, key) = connection(&invocation.environment)?;
        let value = self.exchange(endpoint, key, body).await?;
        *usage = parse_token_usage_delta(value.get("usage"), None, None);
        normalize(value, &contract, &invocation.node.binding, key)
            .map_err(|error| DecisionFailure::new(WorkerErrorCode::Malformed, error.to_string()))
    }

    async fn exchange(
        &self,
        endpoint: Url,
        key: &str,
        body: Vec<u8>,
    ) -> Result<Value, DecisionFailure> {
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(DecisionFailure::transport)?;
        read_response(response).await
    }
}

fn request_body(
    invocation: &DriverInvocation,
    contract: &DecisionContract,
) -> Result<Vec<u8>, NodeRunnerError> {
    let NodeRuntimeBinding::Decision { model, .. } = &invocation.node.binding else {
        return Err(NodeRunnerError::InvalidRole);
    };
    let instructions = invocation
        .node
        .instructions
        .as_ref()
        .ok_or(NodeRunnerError::InvalidRole)?;
    let criteria = contract
        .options
        .values()
        .iter()
        .map(|label| (label.as_str().to_owned(), Value::Null))
        .collect::<BTreeMap<_, _>>();
    let body = serde_json::to_vec(&json!({
        "model": model.as_str(), "state": invocation.node.input,
        "questions": {contract.signal.as_str(): {
            "type": "choice", "instructions": instructions, "criteria": criteria,
        }},
    }))
    .map_err(|_| failure("could not serialize decision request"))?;
    if body.len() > MAX_DECISION_BYTES {
        return Err(failure("decision request exceeds 4 MiB"));
    }
    Ok(body)
}

async fn read_response(mut response: reqwest::Response) -> Result<Value, DecisionFailure> {
    if !response.status().is_success() {
        // Provider error bodies and request URLs may contain credentials. Publish only the status.
        let code = match response.status().as_u16() {
            401 | 403 => WorkerErrorCode::Refusal,
            _ => WorkerErrorCode::Crash,
        };
        return Err(DecisionFailure::new(
            code,
            format!(
                "decision HTTP request returned status {}",
                response.status().as_u16()
            ),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_DECISION_BYTES as u64)
    {
        return Err(DecisionFailure::new(
            WorkerErrorCode::Malformed,
            "decision response exceeds 4 MiB",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(DecisionFailure::transport)? {
        if chunk.len() > MAX_DECISION_BYTES.saturating_sub(bytes.len()) {
            return Err(DecisionFailure::new(
                WorkerErrorCode::Malformed,
                "decision response exceeds 4 MiB",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| {
        DecisionFailure::new(
            WorkerErrorCode::Malformed,
            "decision response is not valid JSON",
        )
    })
}

fn connection(environment: &ResolvedEnvironment) -> Result<(Url, &str), NodeRunnerError> {
    let value = |name: &str| {
        EnvironmentVariableName::new(name)
            .ok()
            .and_then(|name| environment.get(&name))
            .filter(|value| !value.trim().is_empty() && !value.chars().any(char::is_control))
            .ok_or_else(|| failure("decision connection requires a valid endpoint and API key"))
    };
    let endpoint = value(ENDPOINT)?;
    crate::native_v2_capsule::gateway::validate_base_url(endpoint)
        .map_err(|_| failure("decision endpoint must be an absolute HTTP(S) URL without credentials, query or fragment"))?;
    let url = Url::parse(endpoint).map_err(|_| failure("invalid decision endpoint"))?;
    Ok((url, value(API_KEY)?))
}

#[derive(Deserialize)]
struct DecisionResponse {
    model: String,
    answers: BTreeMap<String, ChoiceAnswer>,
}

#[derive(Deserialize)]
struct ChoiceAnswer {
    #[serde(rename = "type")]
    kind: String,
    choice: String,
    probabilities: BTreeMap<String, f64>,
}

fn normalize(
    value: Value,
    contract: &DecisionContract,
    binding: &NodeRuntimeBinding,
    key: &str,
) -> Result<WorkerOutcome, NodeRunnerError> {
    let NodeRuntimeBinding::Decision {
        min_probability,
        uncertain_label,
        ..
    } = binding
    else {
        return Err(NodeRunnerError::InvalidRole);
    };
    let (model, answer) = choice_response(value, contract, key)?;
    let probability = answer.probabilities[&answer.choice];
    let tied = answer
        .probabilities
        .values()
        .filter(|value| **value == probability)
        .count()
        > 1;
    let uncertain = probability < min_probability.get() || tied;
    let label = if uncertain {
        uncertain_label.clone()
    } else {
        EnumLabel::new(answer.choice.clone()).map_err(|_| failure("invalid decision label"))?
    };
    Ok(WorkerOutcome::Verifier {
        output: json!({"choice": answer.choice, "probabilities": answer.probabilities, "model": model}),
        signals: BTreeMap::from([(contract.signal.clone(), label)]),
        diagnostic: json!({"message": if uncertain {"Decision is uncertain."} else {"Decision met the authored probability threshold."}}),
        artifacts: Vec::new(),
    })
}

fn choice_response(
    value: Value,
    contract: &DecisionContract,
    key: &str,
) -> Result<(String, ChoiceAnswer), NodeRunnerError> {
    let mut response: DecisionResponse = serde_json::from_value(value)
        .map_err(|_| failure("decision response does not match the System One choice schema"))?;
    if response.answers.len() != 1
        || ModelId::new(response.model.clone()).is_err()
        || response.model.contains(key)
    {
        return Err(failure(
            "decision response contains an invalid model or answer set",
        ));
    }
    let answer = response
        .answers
        .remove(contract.signal.as_str())
        .ok_or_else(|| failure("decision response is missing its requested answer"))?;
    validate_answer(&answer, contract)?;
    Ok((response.model, answer))
}

fn validate_answer(
    answer: &ChoiceAnswer,
    contract: &DecisionContract,
) -> Result<(), NodeRunnerError> {
    if answer.kind != "choice"
        || answer.probabilities.len() != contract.options.values().len()
        || !contract
            .options
            .values()
            .iter()
            .all(|label| answer.probabilities.contains_key(label.as_str()))
        || !answer.probabilities.contains_key(&answer.choice)
        || answer
            .probabilities
            .values()
            .any(|p| !p.is_finite() || !(0.0..=1.0).contains(p))
    {
        return Err(failure(
            "decision answer has an invalid type, option or probability distribution",
        ));
    }
    let sum: f64 = answer.probabilities.values().sum();
    let chosen = answer.probabilities[&answer.choice];
    if (sum - 1.0).abs() > DISTRIBUTION_TOLERANCE
        || answer.probabilities.values().any(|p| *p > chosen)
    {
        return Err(failure(
            "decision answer probabilities are incomplete or disagree with its choice",
        ));
    }
    Ok(())
}

struct DecisionSession(AtomicBool);

#[async_trait]
impl NodeSession for DecisionSession {
    fn as_any(&self) -> &dyn Any {
        self
    }
    async fn is_live(&self) -> bool {
        !self.0.load(Ordering::Acquire)
    }
    async fn close(&self) {
        self.0.store(true, Ordering::Release);
    }
}

#[async_trait]
impl SessionFactory for DecisionAdapter {
    async fn open(
        &self,
        invocation: &NodeInvocation,
        environment: &ResolvedEnvironment,
    ) -> Result<Arc<dyn NodeSession>, NodeRunnerError> {
        if !matches!(invocation.binding, NodeRuntimeBinding::Decision { .. }) {
            return Err(NodeRunnerError::InvalidRole);
        }
        connection(environment)?;
        Ok(Arc::new(DecisionSession(AtomicBool::new(false))))
    }
}

#[async_trait]
impl NodeDriver for DecisionAdapter {
    async fn run(
        &self,
        invocation: DriverInvocation,
        mut control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let mut usage = None;
        let result = tokio::select! {
            biased;
            () = control.cancelled() => Err(NodeRunnerError::Cancelled),
            result = tokio::time::timeout(REQUEST_TIMEOUT, self.evaluate(&invocation, &mut usage)) => {
                Ok(result.unwrap_or_else(|_| Err(DecisionFailure::new(
                    WorkerErrorCode::Timeout,
                    "decision evaluation timed out",
                ))))
            }
        };
        control.record_token_usage(usage).await?;
        match result? {
            Ok(outcome) => Ok(outcome),
            Err(failure) => {
                control
                    .emit_final(LiveOutput::new(LiveOutputStream::Error, failure.message)?)
                    .await?;
                Ok(match failure.code {
                    WorkerErrorCode::Refusal => WorkerOutcome::authentication_refusal(),
                    WorkerErrorCode::Malformed => WorkerOutcome::malformed(),
                    code => WorkerOutcome::declared_failure(code),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests;
