//! Host-owned human requests. No provider, session, timer or separate inbox storage.
use openengine_cluster_protocol::{
    ContractValueError, FieldName, PayloadType, RecordField, RequestAnswer, RequestResolution,
    RunResumeFrom, RunSuspension, UnixTimestampMillis, WorkerErrorCode, WorkerOutcome,
};
use serde_json::json;

pub const ASK_WORKER_REF: &str = "builtin.request.ask@1";
pub const DEFAULT_DEADLINE_MILLIS: u64 = 7 * 24 * 60 * 60 * 1000;
pub const MAX_DEADLINE_MILLIS: u64 = 30 * 24 * 60 * 60 * 1000;

pub fn input_schema() -> Result<PayloadType, ContractValueError> {
    Ok(PayloadType::Record {
        fields: [(
            FieldName::new("text")?,
            RecordField {
                value_type: PayloadType::String,
                required: true,
            },
        )]
        .into_iter()
        .collect(),
    })
}

pub fn valid_output(output: &PayloadType) -> bool {
    matches!(output, PayloadType::Record { fields } if fields.values().all(|field| matches!(field.value_type,
        PayloadType::Boolean | PayloadType::Integer | PayloadType::Number | PayloadType::String | PayloadType::Enum { .. })))
}

pub fn deadline(
    now: UnixTimestampMillis,
    timeout: Option<u64>,
) -> Result<UnixTimestampMillis, String> {
    let duration = timeout.unwrap_or(DEFAULT_DEADLINE_MILLIS);
    if duration == 0 || duration > MAX_DEADLINE_MILLIS {
        return Err("request timeoutMs must be at most 30 days".to_owned());
    }
    now.get()
        .checked_add(duration)
        .and_then(|value| UnixTimestampMillis::new(value).ok())
        .ok_or_else(|| "request deadline overflow".to_owned())
}

/// Parse a file result with stable JSON paths for every structural refusal.
pub(crate) fn parse_answer(bytes: &[u8]) -> Result<RequestAnswer, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| format!("$: {error}"))?;
    let object = value.as_object().ok_or("$: answer must be an object")?;
    for name in object.keys() {
        if name != "signals" && name != "output" {
            return Err(format!("$.{name} is undeclared"));
        }
    }
    let signals = object
        .get("signals")
        .and_then(serde_json::Value::as_object)
        .ok_or("$.signals must be an object")?;
    let signals = signals
        .iter()
        .map(|(name, label)| {
            let path = format!("$.signals.{name}");
            let key = FieldName::new(name).map_err(|error| format!("{path}: {error}"))?;
            let label = label
                .as_str()
                .ok_or_else(|| format!("{path} must be a label string"))?;
            let label = openengine_cluster_protocol::EnumLabel::new(label)
                .map_err(|error| format!("{path}: {error}"))?;
            Ok((key, label))
        })
        .collect::<Result<_, String>>()?;
    let output = object
        .get("output")
        .cloned()
        .ok_or("$.output is required")?;
    Ok(RequestAnswer { signals, output })
}

/// Validate before resolving credentials or allocating a successor. Deadline precedes shape.
/// Answer callers report an expired resolution as a refusal after admitting its successor.
pub fn resolve(
    suspension: &RunSuspension,
    from: Option<&RunResumeFrom>,
    now: UnixTimestampMillis,
) -> Result<RequestResolution, String> {
    let RunSuspension::Request { request, .. } = suspension else {
        return Err(format!(
            "answer requires a request suspension; run is suspended for {}",
            if suspension.due_at().is_some() {
                "wait"
            } else {
                "command"
            }
        ));
    };
    if !matches!(
        from,
        Some(RunResumeFrom::Answer { .. } | RunResumeFrom::Expiry {})
    ) {
        return Err("request suspension requires zeroshot answer RUN_ID --input FILE".to_owned());
    }
    if now >= request.expires_at {
        return Ok(RequestResolution::Expiry {});
    }
    let Some(RunResumeFrom::Answer { signals, output }) = from else {
        return Err("request has not expired".to_owned());
    };
    for (name, labels) in &request.signals {
        let label = signals
            .get(name)
            .ok_or_else(|| format!("$.signals.{} is required", name.as_str()))?;
        if !labels.values().contains(label) {
            return Err(format!(
                "$.signals.{} has an undeclared label",
                name.as_str()
            ));
        }
    }
    for name in signals.keys() {
        if !request.signals.contains_key(name) {
            return Err(format!("$.signals.{} is undeclared", name.as_str()));
        }
    }
    request
        .output
        .validate_value(output)
        .map_err(|error| format!("$.output: {error}"))?;
    let resolution = RequestResolution::Answer {
        answer: RequestAnswer {
            signals: signals.clone(),
            output: output.clone(),
        },
    };
    resolution
        .outcome()
        .validate()
        .map_err(|error| format!("$: {error}"))?;
    Ok(resolution)
}

impl RequestOutcome for RequestResolution {
    fn outcome(&self) -> WorkerOutcome {
        match self {
            Self::Answer { answer } => WorkerOutcome::Verifier {
                signals: answer.signals.clone(),
                output: answer.output.clone(),
                diagnostic: json!({"message":"request answered"}),
                artifacts: Vec::new(),
            },
            Self::Expiry {} => WorkerOutcome::declared_failure(WorkerErrorCode::Timeout),
        }
    }
}

pub(crate) trait RequestOutcome {
    fn outcome(&self) -> WorkerOutcome;
}

pub(crate) fn find<'a>(
    root: &'a openengine_cluster_protocol::GraphNode,
    name: &openengine_cluster_protocol::NodeName,
) -> Option<&'a openengine_cluster_protocol::VerifierNode> {
    if let openengine_cluster_protocol::GraphNode::Verifier(node) = root
        && &node.name == name
    {
        return Some(node);
    }
    openengine_cluster_server::graph_verifier::graph_node_children(root)
        .into_iter()
        .find_map(|child| find(child, name))
}

pub(crate) fn resolution_vector(
    request: &openengine_cluster_protocol::RunRequest,
    resolution: &RequestResolution,
) -> openengine_cluster_protocol::RunVectorContent {
    match resolution {
        RequestResolution::Answer { answer } => {
            openengine_cluster_protocol::RunVectorContent::Answer {
                request_id: request.id.clone(),
                answer: answer.clone(),
            }
        }
        RequestResolution::Expiry {} => openengine_cluster_protocol::RunVectorContent::Expiry {
            request_id: request.id.clone(),
        },
    }
}

/// Check the same durable result envelope used by executable nodes before claiming a successor.
pub(crate) fn validate_resolution(
    snapshot: &crate::v2_run_ledger::RunSnapshot,
    resolution: &RequestResolution,
) -> Result<(), String> {
    use crate::native_v2_contract::NodeCompletion;
    use crate::v2_run_ledger::{NodeState, RunEvent};
    let pending = snapshot
        .executions
        .values()
        .find(|node| matches!(node.state, NodeState::SuspendedRequest { .. }))
        .ok_or("request has no pending execution")?;
    let completion = RunEvent::NodeCompleted {
        completion: NodeCompletion {
            reference: pending.reference.clone(),
            outcome: resolution.outcome(),
        },
    };
    crate::v2_run_ledger::validate_event(&completion)
        .map_err(|error| format!("$.output: {error}"))?;
    let RunSuspension::Request { request, .. } =
        snapshot.suspension.as_ref().ok_or("run has no request")?
    else {
        return Err("run has no request".to_owned());
    };
    let vector = crate::v2_run_ledger::vector_event(
        &snapshot.run_id,
        resolution_vector(request, resolution),
    )
    .map_err(|error| format!("$.output: {error}"))?;
    crate::v2_run_ledger::validate_event(&vector).map_err(|error| format!("$.output: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use openengine_cluster_testkit::assertions::AssertValue;
    fn suspension() -> RunSuspension {
        serde_json::from_value(json!({"reason":"request","node":"approval","createdAt":1000,
            "request":{"id":"request-1","text":"Review?","signals":{"decision":["approved","rejected"]},
            "output":{"kind":"record","fields":{"feedback":{"required":true,"type":{"kind":"string"}}}},"expiresAt":2000}})).assert_value()
    }
    fn answer(value: serde_json::Value) -> RunResumeFrom {
        serde_json::from_value(value).assert_value()
    }
    #[test]
    fn deadline_precedes_shape_and_is_strict() {
        let valid = answer(
            json!({"kind":"answer","signals":{"decision":"approved"},"output":{"feedback":"ok"}}),
        );
        assert!(matches!(
            resolve(
                &suspension(),
                Some(&valid),
                UnixTimestampMillis::new(1999).assert_value()
            )
            .assert_value(),
            RequestResolution::Answer { .. }
        ));
        for selection in [
            valid,
            answer(json!({"kind":"answer","signals":{},"output":null})),
            RunResumeFrom::Expiry {},
        ] {
            assert_eq!(
                resolve(
                    &suspension(),
                    Some(&selection),
                    UnixTimestampMillis::new(2000).assert_value()
                )
                .assert_value(),
                RequestResolution::Expiry {}
            );
        }
        assert!(
            resolve(
                &suspension(),
                Some(&RunResumeFrom::Expiry {}),
                UnixTimestampMillis::new(1999).assert_value()
            )
            .is_err()
        );
        assert_eq!(
            deadline(UnixTimestampMillis::new(1000).assert_value(), None)
                .assert_value()
                .get(),
            1000 + DEFAULT_DEADLINE_MILLIS
        );
        assert!(deadline(UnixTimestampMillis::MIN, Some(MAX_DEADLINE_MILLIS)).is_ok());
        assert!(deadline(UnixTimestampMillis::MIN, Some(MAX_DEADLINE_MILLIS + 1)).is_err());
    }
    #[test]
    fn invalid_answers_name_the_offending_path() {
        for (signals, output, path) in [
            (json!({}), json!({"feedback":"ok"}), "signals.decision"),
            (
                json!({"decision":"other"}),
                json!({"feedback":"ok"}),
                "signals.decision",
            ),
            (
                json!({"decision":"approved","other":"x"}),
                json!({"feedback":"ok"}),
                "signals.other",
            ),
            (json!({"decision":"approved"}), json!({}), "feedback"),
            (
                json!({"decision":"approved"}),
                json!({"feedback":1}),
                "feedback",
            ),
            (
                json!({"decision":"approved"}),
                json!({"feedback":"ok","extra":true}),
                "extra",
            ),
        ] {
            let from = answer(json!({"kind":"answer","signals":signals,"output":output}));
            let error = resolve(
                &suspension(),
                Some(&from),
                UnixTimestampMillis::new(1999).assert_value(),
            )
            .unwrap_err();
            assert!(error.contains(path), "{error}");
        }
    }
}
