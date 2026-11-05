use std::{collections::BTreeMap, fmt};

use openengine_cluster_protocol::{
    EnumLabel, FieldName, NodeInstructions, NonEmptyEnumSet, PayloadType, WorkerOutcome,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{DriverControl, LiveOutput, LiveOutputStream, NodeRunnerError};

#[path = "response/guidance.rs"]
mod guidance;

const MAX_RESPONSE_ERROR_BYTES: usize = 8 * 1024;
const MAX_OUTPUT_CORRECTIONS: usize = 2;
const OPENAI_OPTIONAL_NULL_OMISSION: &str = "__zeroshot_omitted_optional_null__";
const EXPECTED_SHAPE_HEADER: &str = "Expected shape (angle-bracket placeholders are not JSON; \
     replace each with a real value; \"name\"? marks an optional field; the schema or contract above \
     stays authoritative):";
const START_REPLY_SENTENCE: &str = "Start the reply with the opening brace and end it with the final \
     closing brace of that one object.";

/// Ephemeral response contract derived from the admitted graph leaf.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeResponseContract {
    Worker {
        output: PayloadType,
    },
    Verifier {
        output: PayloadType,
        signals: BTreeMap<FieldName, NonEmptyEnumSet>,
        diagnostic: PayloadType,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderSchemaDialect {
    Standard,
    OpenAiStrict,
}

impl NodeResponseContract {
    pub(crate) fn provider_schema(&self, dialect: ProviderSchemaDialect) -> Value {
        closed_object_schema(
            BTreeMap::from([("response".to_owned(), self.response_schema(dialect))]),
            vec!["response".to_owned()],
        )
    }

    fn response_schema(&self, dialect: ProviderSchemaDialect) -> Value {
        match self {
            Self::Worker { output } => payload_schema(output, dialect),
            Self::Verifier {
                output,
                signals,
                diagnostic,
            } => {
                let signal_properties = signals
                    .iter()
                    .map(|(name, labels)| {
                        (
                            name.as_str().to_owned(),
                            enum_schema(labels.values().iter().map(EnumLabel::as_str)),
                        )
                    })
                    .collect::<BTreeMap<_, _>>();
                closed_object_schema(
                    BTreeMap::from([
                        ("diagnostic".to_owned(), payload_schema(diagnostic, dialect)),
                        ("output".to_owned(), payload_schema(output, dialect)),
                        (
                            "signals".to_owned(),
                            closed_object_schema(
                                signal_properties,
                                signals
                                    .keys()
                                    .map(|name| name.as_str().to_owned())
                                    .collect(),
                            ),
                        ),
                    ]),
                    vec![
                        "diagnostic".to_owned(),
                        "output".to_owned(),
                        "signals".to_owned(),
                    ],
                )
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn parse_agent_response(
        &self,
        response: &str,
    ) -> Result<WorkerOutcome, NodeResponseError> {
        let value = serde_json::from_str(response).map_err(|error| {
            NodeResponseError::new(format!("final response is not valid JSON: {error}"))
        })?;
        self.parse_agent_value(value)
    }

    pub(crate) fn parse_agent_value(
        &self,
        value: Value,
    ) -> Result<WorkerOutcome, NodeResponseError> {
        let outcome = match self {
            Self::Worker { .. } => WorkerOutcome::Verified {
                output: value,
                artifacts: Vec::new(),
            },
            Self::Verifier { .. } => {
                let verifier: VerifierResponse = serde_json::from_value(value).map_err(|error| {
                    NodeResponseError::new(format!(
                        "verifier response must contain exactly output, signals, and diagnostic: {error}"
                    ))
                })?;
                WorkerOutcome::Verifier {
                    output: verifier.output,
                    signals: verifier.signals,
                    diagnostic: verifier.diagnostic,
                    artifacts: Vec::new(),
                }
            }
        };
        self.validate_agent_outcome(&outcome)?;
        Ok(outcome)
    }

    fn normalize_openai_response(&self, value: &mut Value) {
        match self {
            Self::Worker { output } => normalize_openai_payload(output, value),
            Self::Verifier {
                output, diagnostic, ..
            } => {
                let Some(response) = value.as_object_mut() else {
                    return;
                };
                if let Some(output_value) = response.get_mut("output") {
                    normalize_openai_payload(output, output_value);
                }
                if let Some(diagnostic_value) = response.get_mut("diagnostic") {
                    normalize_openai_payload(diagnostic, diagnostic_value);
                }
            }
        }
    }

    pub(super) fn validate_outcome(&self, outcome: &WorkerOutcome) -> Result<(), NodeRunnerError> {
        validate_native_v2_outcome(outcome)?;
        if matches!(outcome, WorkerOutcome::Error { .. }) {
            return Ok(());
        }
        self.validate_agent_outcome(outcome)
            .map_err(|_| NodeRunnerError::Driver)
    }

    fn validate_agent_outcome(&self, outcome: &WorkerOutcome) -> Result<(), NodeResponseError> {
        match (self, outcome) {
            (Self::Worker { output: expected }, WorkerOutcome::Verified { output, .. }) => expected
                .validate_value(output)
                .map_err(|error| NodeResponseError::new(format!("output {error}"))),
            (
                Self::Verifier {
                    output: expected_output,
                    signals: expected_signals,
                    diagnostic: expected_diagnostic,
                },
                WorkerOutcome::Verifier {
                    output,
                    signals,
                    diagnostic,
                    ..
                },
            ) => {
                expected_output
                    .validate_value(output)
                    .map_err(|error| NodeResponseError::new(format!("output {error}")))?;
                expected_diagnostic
                    .validate_value(diagnostic)
                    .map_err(|error| NodeResponseError::new(format!("diagnostic {error}")))?;
                validate_signals(expected_signals, signals)
            }
            _ => Err(NodeResponseError::new(
                "final response does not match the node role".to_owned(),
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NodeResponseError(Box<str>);

impl NodeResponseError {
    /// Complete transient diagnostic, for credential redaction before public truncation.
    pub(crate) fn diagnostic(&self) -> &str {
        &self.0
    }

    fn new(message: String) -> Self {
        // Keep original text transiently until the provider can redact credentials. Display
        // bounds correction prompts; public diagnostics are bounded only after redaction.
        Self(message.into_boxed_str())
    }
}

pub(crate) enum AgentResponse {
    Complete(WorkerOutcome),
    Correction {
        prompt: String,
        diagnostic: NodeResponseError,
    },
}

impl AgentResponse {
    pub(crate) fn correction_error(&self) -> Option<NodeRunnerError> {
        match self {
            Self::Correction { diagnostic, .. } => Some(NodeRunnerError::DriverDetail(format!(
                "final output rejected: {}",
                diagnostic.0
            ))),
            Self::Complete(_) => None,
        }
    }
}

pub(crate) struct AgentResponseState {
    prompt: String,
    corrections: usize,
}

impl AgentResponseState {
    pub(crate) fn new(prompt: String) -> Self {
        Self {
            prompt,
            corrections: 0,
        }
    }

    pub(crate) fn prompt(&self) -> &str {
        &self.prompt
    }

    pub(crate) fn corrections_exhausted(&self) -> bool {
        self.corrections == MAX_OUTPUT_CORRECTIONS
    }

    pub(crate) fn replace_prompt(&mut self, prompt: String) {
        self.prompt = prompt;
    }

    pub(crate) async fn accept(
        &mut self,
        provider: &str,
        control: &DriverControl,
        response: AgentResponse,
    ) -> Result<Option<WorkerOutcome>, NodeRunnerError> {
        match response {
            AgentResponse::Complete(outcome) => Ok(Some(outcome)),
            AgentResponse::Correction { .. } if self.corrections_exhausted() => {
                control
                    .emit(LiveOutput::new(
                        LiveOutputStream::System,
                        format!("{provider} final output remained malformed after two corrections"),
                    )?)
                    .await?;
                Ok(Some(WorkerOutcome::malformed()))
            }
            AgentResponse::Correction { prompt, .. } => {
                self.corrections += 1;
                self.prompt = prompt;
                Ok(None)
            }
        }
    }
}

pub(crate) fn resolve_agent_response(
    contract: &NodeResponseContract,
    response: &str,
) -> Result<AgentResponse, NodeRunnerError> {
    resolve_agent_response_with_dialect(contract, response, ProviderSchemaDialect::Standard)
}

pub(crate) fn resolve_agent_response_with_dialect(
    contract: &NodeResponseContract,
    response: &str,
    dialect: ProviderSchemaDialect,
) -> Result<AgentResponse, NodeRunnerError> {
    resolve_agent_response_rendering(contract, response, dialect, CorrectionRendering::Contract)
}

/// Resolves a response whose prompt carried the provider schema rather than the ephemeral contract
/// object, so a rejection restates that same schema instead of the contract.
pub(crate) fn resolve_agent_response_with_schema(
    contract: &NodeResponseContract,
    response: &str,
    dialect: ProviderSchemaDialect,
) -> Result<AgentResponse, NodeRunnerError> {
    resolve_agent_response_rendering(
        contract,
        response,
        dialect,
        CorrectionRendering::Schema(dialect),
    )
}

/// Which response representation a correction turn restates. The initial prompt and every
/// correction must agree so the provider never sees the contract and the schema together.
#[derive(Clone, Copy)]
enum CorrectionRendering {
    Contract,
    Schema(ProviderSchemaDialect),
}

fn resolve_agent_response_rendering(
    contract: &NodeResponseContract,
    response: &str,
    dialect: ProviderSchemaDialect,
    correction: CorrectionRendering,
) -> Result<AgentResponse, NodeRunnerError> {
    let (parsed, fault) = match parse_provider_envelope(response) {
        Ok(mut semantic_response) => {
            if dialect == ProviderSchemaDialect::OpenAiStrict {
                contract.normalize_openai_response(&mut semantic_response);
            }
            (contract.parse_agent_value(semantic_response), None)
        }
        Err(error) => (Err(error), envelope_fault(response)),
    };
    Ok(match parsed {
        Ok(outcome) => AgentResponse::Complete(outcome),
        Err(error) => {
            let prompt = match correction {
                CorrectionRendering::Contract => render_agent_correction(contract, &error, fault)?,
                CorrectionRendering::Schema(dialect) => {
                    render_agent_correction_with_schema(contract, &error, fault, dialect)?
                }
            };
            AgentResponse::Correction {
                prompt,
                diagnostic: error,
            }
        }
    })
}

fn parse_provider_envelope(response: &str) -> Result<Value, NodeResponseError> {
    let envelope: ProviderResponseEnvelope = serde_json::from_str(response).map_err(|error| {
        NodeResponseError::new(format!(
            "provider response must be exactly an object containing response: {error}"
        ))
    })?;
    Ok(envelope.response)
}

/// Envelope-level syntactic fault, detected only after the envelope failed to parse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EnvelopeFault {
    Empty,
    LeadingText,
    Unterminated,
    TrailingContent,
}

impl EnvelopeFault {
    fn problem(self) -> &'static str {
        match self {
            Self::Empty => "The reply was empty. Return the JSON object shown below.",
            Self::LeadingText => {
                "The reply began with text before the JSON object. Start the reply with the opening \
                 brace of the object; put no explanation, heading, or code fence before it."
            }
            Self::Unterminated => {
                "The reply ended before the JSON object was complete. Close every object and array \
                 you open; the expected shape below shows every closing brace the reply must end \
                 with."
            }
            Self::TrailingContent => {
                "The reply continued after one complete JSON object, for example with a second copy \
                 of it. Return the object exactly once and nothing after it."
            }
        }
    }
}

fn envelope_fault(response: &str) -> Option<EnvelopeFault> {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return Some(EnvelopeFault::Empty);
    }
    if !trimmed.starts_with('{') {
        return Some(EnvelopeFault::LeadingText);
    }
    let mut stream = serde_json::Deserializer::from_str(trimmed).into_iter::<Value>();
    match stream.next() {
        Some(Err(error)) if error.classify() == serde_json::error::Category::Eof => {
            Some(EnvelopeFault::Unterminated)
        }
        Some(Ok(_)) if stream.byte_offset() < trimmed.len() => Some(EnvelopeFault::TrailingContent),
        _ => None,
    }
}

/// One-line skeleton of the expected provider response. Angle-bracket placeholders are not JSON,
/// so copying the skeleton verbatim can never pass validation.
fn response_skeleton(contract: &NodeResponseContract) -> Result<String, NodeRunnerError> {
    let mut out = String::from("{\"response\":");
    match contract {
        NodeResponseContract::Worker { output } => payload_skeleton(output, &mut out)?,
        NodeResponseContract::Verifier {
            output,
            signals,
            diagnostic,
        } => {
            out.push_str("{\"output\":");
            payload_skeleton(output, &mut out)?;
            out.push_str(",\"signals\":{");
            for (index, (name, labels)) in signals.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&json_string(name.as_str())?);
                out.push_str(":<one of ");
                for (label_index, label) in labels.values().iter().enumerate() {
                    if label_index > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&json_string(label.as_str())?);
                }
                out.push('>');
            }
            out.push_str("},\"diagnostic\":");
            payload_skeleton(diagnostic, &mut out)?;
            out.push('}');
        }
    }
    out.push('}');
    Ok(out)
}

fn payload_skeleton(payload: &PayloadType, out: &mut String) -> Result<(), NodeRunnerError> {
    match payload {
        PayloadType::Null => out.push_str("null"),
        PayloadType::Boolean => out.push_str("<boolean>"),
        PayloadType::Integer => out.push_str("<integer>"),
        PayloadType::Number => out.push_str("<number>"),
        PayloadType::String => out.push_str("<string>"),
        PayloadType::Record { fields } => {
            out.push('{');
            for (index, (name, field)) in fields.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&json_string(name.as_str())?);
                if !field.required {
                    out.push('?');
                }
                out.push(':');
                payload_skeleton(&field.value_type, out)?;
            }
            out.push('}');
        }
        PayloadType::Array { items } => {
            out.push('[');
            payload_skeleton(items, out)?;
            out.push_str(",...]");
        }
        PayloadType::Enum { values } => {
            out.push_str("<one of ");
            for (index, label) in values.values().iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&json_string(label.as_str())?);
            }
            out.push('>');
        }
    }
    Ok(())
}

fn json_string(value: &str) -> Result<String, NodeRunnerError> {
    serde_json::to_string(value).map_err(|_| NodeRunnerError::Driver)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderResponseEnvelope {
    response: Value,
}

impl fmt::Display for NodeResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.len() <= MAX_RESPONSE_ERROR_BYTES {
            return formatter.write_str(&self.0);
        }
        let mut end = MAX_RESPONSE_ERROR_BYTES.saturating_sub(3);
        while !self.0.is_char_boundary(end) {
            end -= 1;
        }
        formatter.write_str(&self.0[..end])?;
        formatter.write_str("...")
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifierResponse {
    output: Value,
    signals: BTreeMap<FieldName, EnumLabel>,
    diagnostic: Value,
}

fn validate_signals(
    expected: &BTreeMap<FieldName, NonEmptyEnumSet>,
    actual: &BTreeMap<FieldName, EnumLabel>,
) -> Result<(), NodeResponseError> {
    for name in expected.keys() {
        if !actual.contains_key(name) {
            return Err(NodeResponseError::new(format!(
                "signals is missing required field {}",
                name.as_str()
            )));
        }
    }
    for (name, label) in actual {
        let Some(labels) = expected.get(name) else {
            return Err(NodeResponseError::new(format!(
                "signals contains undeclared field {}",
                name.as_str()
            )));
        };
        if !labels.values().iter().any(|allowed| allowed == label) {
            return Err(NodeResponseError::new(format!(
                "signals.{} contains undeclared label {}",
                name.as_str(),
                label.as_str()
            )));
        }
    }
    Ok(())
}

fn validate_native_v2_outcome(outcome: &WorkerOutcome) -> Result<(), NodeRunnerError> {
    outcome.validate().map_err(|_| NodeRunnerError::Driver)?;
    match outcome {
        WorkerOutcome::Verified { artifacts, .. } | WorkerOutcome::Verifier { artifacts, .. }
            if !artifacts.is_empty() =>
        {
            Err(NodeRunnerError::Driver)
        }
        WorkerOutcome::Verified { .. }
        | WorkerOutcome::Verifier { .. }
        | WorkerOutcome::Error { .. } => Ok(()),
    }
}

fn payload_schema(payload: &PayloadType, dialect: ProviderSchemaDialect) -> Value {
    match payload {
        PayloadType::Null => json!({ "type": "null" }),
        PayloadType::Boolean => json!({ "type": "boolean" }),
        PayloadType::Integer => json!({ "type": "integer" }),
        PayloadType::Number => json!({ "type": "number" }),
        PayloadType::String => json!({ "type": "string" }),
        PayloadType::Record { fields } => {
            let properties = fields
                .iter()
                .map(|(name, field)| {
                    let schema = payload_schema(&field.value_type, dialect);
                    let schema =
                        if !field.required && dialect == ProviderSchemaDialect::OpenAiStrict {
                            openai_optional_schema(&field.value_type, schema)
                        } else {
                            schema
                        };
                    (name.as_str().to_owned(), schema)
                })
                .collect::<BTreeMap<_, _>>();
            let required = fields
                .iter()
                .filter(|(_, field)| {
                    field.required || dialect == ProviderSchemaDialect::OpenAiStrict
                })
                .map(|(name, _)| name.as_str().to_owned())
                .collect();
            closed_object_schema(properties, required)
        }
        PayloadType::Array { items } => {
            json!({ "type": "array", "items": payload_schema(items, dialect) })
        }
        PayloadType::Enum { values } => enum_schema(values.values().iter().map(EnumLabel::as_str)),
    }
}

fn openai_optional_schema(payload: &PayloadType, schema: Value) -> Value {
    let omission = match payload {
        PayloadType::Null => json!({
            "type": "string",
            "enum": [OPENAI_OPTIONAL_NULL_OMISSION],
            "description": "Use this sentinel when the optional field is omitted."
        }),
        _ => json!({ "type": "null" }),
    };
    json!({ "anyOf": [schema, omission] })
}

fn openai_value_is_omitted(payload: &PayloadType, value: Option<&Value>) -> bool {
    match payload {
        PayloadType::Null => value.and_then(Value::as_str) == Some(OPENAI_OPTIONAL_NULL_OMISSION),
        _ => value.is_some_and(Value::is_null),
    }
}

fn normalize_openai_payload(payload: &PayloadType, value: &mut Value) {
    match (payload, value) {
        (PayloadType::Record { fields }, Value::Object(values)) => {
            for (name, field) in fields {
                let name = name.as_str();
                if !field.required && openai_value_is_omitted(&field.value_type, values.get(name)) {
                    values.remove(name);
                    continue;
                }
                if let Some(field_value) = values.get_mut(name) {
                    normalize_openai_payload(&field.value_type, field_value);
                }
            }
        }
        (PayloadType::Array { items }, Value::Array(values)) => {
            for item in values {
                normalize_openai_payload(items, item);
            }
        }
        _ => {}
    }
}

fn enum_schema<'a>(values: impl Iterator<Item = &'a str>) -> Value {
    json!({ "type": "string", "enum": values.collect::<Vec<_>>() })
}

fn closed_object_schema(properties: BTreeMap<String, Value>, required: Vec<String>) -> Value {
    let properties = properties.into_iter().collect::<Map<_, _>>();
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

/// Renders a provider-neutral node turn contract for the shared workspace.
pub fn render_agent_prompt(
    instructions: &NodeInstructions,
    input: &Value,
    response: &NodeResponseContract,
) -> Result<String, NodeRunnerError> {
    let runtime_guidance = guidance::runtime_guidance(response);
    let instructions = instructions.as_str();
    let input = serde_json::to_string(input).map_err(|_| NodeRunnerError::Driver)?;
    let response = serde_json::to_string(response).map_err(|_| NodeRunnerError::Driver)?;
    Ok(format!(
        "Execute this graph node using the shared workspace.\n\
         Authored instructions:\n{instructions}\n\
         {runtime_guidance}\
         Input JSON:\n{input}\n\
         Runtime-owned response contract:\n{response}\n\
         The response contract describes the required type; never return the contract itself. \
         Return only JSON with no Markdown or commentary. The provider response must be exactly \
         an object with one field named response. For a worker, response contains the output \
         value; an output contract of {{\"kind\":\"null\"}} requires the literal null inside \
         {{\"response\":null}}. For a verifier, response contains exactly an object with output, \
         signals, and diagnostic; every signal must use one of its declared labels."
    ))
}

/// Renders a provider-neutral node turn with the provider's machine-readable response schema in
/// place of the ephemeral contract object.
///
/// Lanes that have no native schema flag, such as Pi, carry the schema in the prompt itself. The
/// contract object is deliberately omitted because its `kind` discriminator is runtime bookkeeping
/// that providers otherwise copy into their final response.
pub(crate) fn render_agent_prompt_with_schema(
    instructions: &NodeInstructions,
    input: &Value,
    response: &NodeResponseContract,
    dialect: ProviderSchemaDialect,
) -> Result<String, NodeRunnerError> {
    let runtime_guidance = guidance::runtime_guidance(response);
    let instructions = instructions.as_str();
    let input = serde_json::to_string(input).map_err(|_| NodeRunnerError::Driver)?;
    let response_schema = serde_json::to_string(&response.provider_schema(dialect))
        .map_err(|_| NodeRunnerError::Driver)?;
    Ok(format!(
        "Execute this graph node using the shared workspace.\n\
         Authored instructions:\n{instructions}\n\
         {runtime_guidance}\
         Input JSON:\n{input}\n\
         Runtime-owned response schema:\n{response_schema}\n\
         The response schema describes the required JSON value; never return the schema itself. \
         Return only JSON with no Markdown or commentary. The provider response must be exactly \
         an object with one field named response whose value matches the schema's response \
         property. For a worker, response contains the output value; a response property whose \
         type is null requires the literal null inside {{\"response\":null}}. For a verifier, \
         response contains exactly an object with output, signals, and diagnostic; every signal \
         must use one of its declared labels."
    ))
}

/// Renders one mechanical correction turn in the already-open provider session.
pub(crate) fn render_agent_correction(
    response: &NodeResponseContract,
    error: &NodeResponseError,
    fault: Option<EnvelopeFault>,
) -> Result<String, NodeRunnerError> {
    let problem = correction_problem(fault);
    let skeleton = response_skeleton(response)?;
    let response = serde_json::to_string(response).map_err(|_| NodeRunnerError::Driver)?;
    Ok(format!(
        "Your previous final response was rejected mechanically and was not passed to the graph.\n\
         Validation error:\n{error}\n\
         {problem}\
         Response contract:\n{response}\n\
         {EXPECTED_SHAPE_HEADER}\n{skeleton}\n\
         Return a corrected provider response only: exactly an object with one field named \
         response. It must be valid JSON with no Markdown or commentary.\n\
         {START_REPLY_SENTENCE}"
    ))
}

/// Renders one mechanical correction turn in the already-open provider session, restating the
/// provider's machine-readable schema rather than the contract.
pub(crate) fn render_agent_correction_with_schema(
    response: &NodeResponseContract,
    error: &NodeResponseError,
    fault: Option<EnvelopeFault>,
    dialect: ProviderSchemaDialect,
) -> Result<String, NodeRunnerError> {
    let response_schema = serde_json::to_string(&response.provider_schema(dialect))
        .map_err(|_| NodeRunnerError::Driver)?;
    let problem = correction_problem(fault);
    let skeleton = response_skeleton(response)?;
    Ok(format!(
        "Your previous final response was rejected mechanically and was not passed to the graph.\n\
         Validation error:\n{error}\n\
         {problem}\
         Response schema:\n{response_schema}\n\
         {EXPECTED_SHAPE_HEADER}\n{skeleton}\n\
         Return a corrected provider response only: exactly an object with one field named \
         response whose value matches the schema above. It must be valid JSON with no Markdown \
         or commentary.\n\
         {START_REPLY_SENTENCE}"
    ))
}

fn correction_problem(fault: Option<EnvelopeFault>) -> String {
    fault.map_or_else(String::new, |fault| {
        format!("Problem:\n{}\n", fault.problem())
    })
}

#[cfg(test)]
#[path = "response/tests.rs"]
mod tests;
