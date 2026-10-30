use openengine_cluster_protocol::{EnumLabel, FieldName, NonEmptyEnumSet, PayloadType, RecordField};

use crate::native_v2_contract::NodeRuntimeBinding;
use crate::native_v2_runner::{NodeResponseContract, NodeRunnerError};

pub(crate) const ENDPOINT: &str = "DECISION_ENDPOINT";
pub(crate) const API_KEY: &str = "DECISION_API_KEY";

fn failure(message: &str) -> NodeRunnerError {
    NodeRunnerError::DriverDetail(message.to_owned())
}

fn record(fields: impl IntoIterator<Item = (FieldName, PayloadType)>) -> PayloadType {
    PayloadType::Record {
        fields: fields
            .into_iter()
            .map(|(name, value_type)| {
                (
                    name,
                    RecordField {
                        value_type,
                        required: true,
                    },
                )
            })
            .collect(),
    }
}

fn name(value: &str) -> Result<FieldName, NodeRunnerError> {
    FieldName::new(value).map_err(|_| failure("invalid decision field name"))
}

pub(crate) fn output_schema(options: &NonEmptyEnumSet) -> Result<PayloadType, NodeRunnerError> {
    Ok(record([
        (
            name("choice")?,
            PayloadType::Enum {
                values: options.clone(),
            },
        ),
        (
            name("probabilities")?,
            record(
                options
                    .values()
                    .iter()
                    .map(|label| name(label.as_str()).map(|name| (name, PayloadType::Number)))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        ),
        (name("model")?, PayloadType::String),
    ]))
}

pub(crate) fn diagnostic_schema() -> Result<PayloadType, NodeRunnerError> {
    Ok(record([(name("message")?, PayloadType::String)]))
}

pub(crate) struct DecisionContract {
    pub(crate) signal: FieldName,
    pub(crate) options: NonEmptyEnumSet,
}

pub(crate) fn validate_contract(
    response: &NodeResponseContract,
    binding: &NodeRuntimeBinding,
) -> Result<DecisionContract, NodeRunnerError> {
    let NodeRuntimeBinding::Decision {
        uncertain_label,
        connections,
        ..
    } = binding
    else {
        return Err(NodeRunnerError::InvalidRole);
    };
    validate_connections(connections)?;
    let NodeResponseContract::Verifier {
        output,
        signals,
        diagnostic,
    } = response
    else {
        return Err(failure("decision binding requires a verifier"));
    };
    let (signal, options) = decision_options(signals, uncertain_label)?;
    if output != &output_schema(&options)? || diagnostic != &diagnostic_schema()? {
        return Err(failure(
            "decision verifier must declare choice, probabilities and model output and a message diagnostic",
        ));
    }
    Ok(DecisionContract { signal, options })
}

fn validate_connections(
    connections: &openengine_cluster_protocol::DeclaredConnections,
) -> Result<(), NodeRunnerError> {
    let fields = connections
        .environment_names()
        .map(|name| name.as_str())
        .collect::<Vec<_>>();
    if fields.len() == 2 && fields.contains(&ENDPOINT) && fields.contains(&API_KEY) {
        Ok(())
    } else {
        Err(failure(
            "decision connections must declare exactly DECISION_ENDPOINT and DECISION_API_KEY",
        ))
    }
}

fn decision_options(
    signals: &std::collections::BTreeMap<FieldName, NonEmptyEnumSet>,
    uncertain_label: &EnumLabel,
) -> Result<(FieldName, NonEmptyEnumSet), NodeRunnerError> {
    if signals.len() != 1 {
        return Err(failure("decision verifier requires exactly one signal"));
    }
    let (signal, labels) = signals
        .iter()
        .next()
        .ok_or_else(|| failure("missing decision signal"))?;
    if !labels.values().contains(uncertain_label) {
        return Err(failure(
            "decision uncertainLabel must be a declared signal label",
        ));
    }
    let options = labels
        .values()
        .iter()
        .filter(|label| *label != uncertain_label)
        .cloned()
        .collect::<Vec<_>>();
    if options.len() < 2 {
        return Err(failure(
            "decision verifier requires at least two options besides uncertainLabel",
        ));
    }
    let options = NonEmptyEnumSet::new(options).map_err(|_| failure("invalid decision options"))?;
    Ok((signal.clone(), options))
}
