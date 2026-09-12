use openengine_cluster_protocol::{
    ContractValueError, EnumLabel, FieldName, NonEmptyEnumSet, PayloadType, RecordField,
};
use serde::Deserialize;
use serde_json::Value;

use crate::native_v2_runner::{NodeResponseContract, NodeRunnerError};

use super::{
    DELIVERY_CI_FAILED_LABEL, DELIVERY_CONFLICT_LABEL, DELIVERY_HEAD_REVISION_FIELD,
    DELIVERY_MERGED_LABEL, DELIVERY_MERGE_REVISION_FIELD, DELIVERY_MODE_FIELD,
    DELIVERY_OPENED_LABEL, DELIVERY_OUTCOME_FIELD, DELIVERY_PULL_REQUEST_ID_FIELD,
    DELIVERY_REPOSITORY_FIELD, DELIVERY_SIGNAL_FIELD, DELIVERY_TARGET_BRANCH_FIELD,
    DELIVERY_VERSION_FIELD, DELIVERY_REPAIR_REQUIRED_LABEL, DeliveryMode, DeliveryTarget,
    valid_review_id, valid_revision,
};

pub fn validate_delivery_contract(
    mode: DeliveryMode,
    response: &NodeResponseContract,
) -> Result<(), NodeRunnerError> {
    let NodeResponseContract::Verifier {
        output,
        signals,
        diagnostic,
    } = response
    else {
        return Err(NodeRunnerError::InvalidRole);
    };
    if !delivery_contract_matches(mode, output, signals, diagnostic)
        .map_err(|_| NodeRunnerError::Driver)?
    {
        return Err(NodeRunnerError::Driver);
    }
    Ok(())
}

fn delivery_contract_matches(
    mode: DeliveryMode,
    output: &PayloadType,
    signals: &std::collections::BTreeMap<FieldName, NonEmptyEnumSet>,
    diagnostic: &PayloadType,
) -> Result<bool, ContractValueError> {
    let field = FieldName::new(DELIVERY_SIGNAL_FIELD)?;
    let matching = [true, false]
        .into_iter()
        .map(|repair| {
            Ok(output == &result_schema(mode, repair)?
                && signals.get(&field) == Some(&signal_labels(mode, repair)?))
        })
        .collect::<Result<Vec<_>, ContractValueError>>()?
        .into_iter()
        .any(|matches| matches);
    Ok(matching && diagnostic == &delivery_diagnostic_schema()? && signals.len() == 1)
}

pub(crate) fn delivery_diagnostic_schema() -> Result<PayloadType, ContractValueError> {
    Ok(PayloadType::Record {
        fields: [contract_field("message", PayloadType::String)?]
            .into_iter()
            .collect(),
    })
}

#[must_use]
pub fn is_matching_success_receipt(
    output: &Value,
    mode: DeliveryMode,
    target: &DeliveryTarget,
) -> bool {
    let Ok(result) = serde_json::from_value::<DeliveryResultWire>(output.clone()) else {
        return false;
    };
    result.version == mode.result_version()
        && result.mode == mode.label()
        && result.outcome == mode.success_outcome()
        && receipt_matches_target(&result, mode, target)
}

fn receipt_matches_target(
    result: &DeliveryResultWire,
    mode: DeliveryMode,
    target: &DeliveryTarget,
) -> bool {
    result.repository == target.repository
        && result.target_branch == target.target_branch
        && valid_revision(&result.head_revision)
        && result.head_revision != target.base_revision
        && valid_review_id(&result.pull_request_id)
        && match mode {
            DeliveryMode::PullRequest => result.merge_revision.is_none(),
            DeliveryMode::MergeV1 => result.merge_revision.is_none(),
            DeliveryMode::Merge => result.merge_revision.as_deref().is_some_and(valid_revision),
        }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeliveryResultWire {
    version: String,
    mode: String,
    outcome: String,
    repository: String,
    target_branch: String,
    head_revision: String,
    pull_request_id: String,
    merge_revision: Option<String>,
}

pub(crate) fn delivery_result_schema(
    mode: DeliveryMode,
) -> Result<PayloadType, ContractValueError> {
    result_schema(mode, true)
}

fn result_schema(mode: DeliveryMode, repair: bool) -> Result<PayloadType, ContractValueError> {
    let mut fields = delivery_identity_fields(mode)?;
    fields.extend(delivery_mode_fields(mode, repair)?);
    Ok(PayloadType::Record {
        fields: fields.into_iter().collect(),
    })
}

fn delivery_identity_fields(
    mode: DeliveryMode,
) -> Result<Vec<(FieldName, RecordField)>, ContractValueError> {
    let mut fields = vec![
        contract_field(
            DELIVERY_VERSION_FIELD,
            contract_enum(&[mode.result_version()])?,
        )?,
        contract_field(DELIVERY_REPOSITORY_FIELD, PayloadType::String)?,
        contract_field(DELIVERY_TARGET_BRANCH_FIELD, PayloadType::String)?,
        contract_field(DELIVERY_HEAD_REVISION_FIELD, PayloadType::String)?,
        contract_field(DELIVERY_PULL_REQUEST_ID_FIELD, PayloadType::String)?,
    ];
    if mode.includes_merge_revision() {
        fields.push(contract_field(
            DELIVERY_MERGE_REVISION_FIELD,
            PayloadType::String,
        )?);
    }
    Ok(fields)
}

fn delivery_mode_fields(
    mode: DeliveryMode,
    repair: bool,
) -> Result<Vec<(FieldName, RecordField)>, ContractValueError> {
    Ok(vec![
        contract_field(DELIVERY_MODE_FIELD, contract_enum(&[mode.label()])?)?,
        contract_field(
            DELIVERY_OUTCOME_FIELD,
            PayloadType::Enum {
                values: signal_labels(mode, repair)?,
            },
        )?,
    ])
}

pub(crate) fn delivery_signal_labels(
    mode: DeliveryMode,
) -> Result<NonEmptyEnumSet, ContractValueError> {
    signal_labels(mode, true)
}

fn signal_labels(mode: DeliveryMode, repair: bool) -> Result<NonEmptyEnumSet, ContractValueError> {
    let mut labels = match mode {
        DeliveryMode::PullRequest => vec![DELIVERY_OPENED_LABEL],
        DeliveryMode::MergeV1 | DeliveryMode::Merge => vec![
            DELIVERY_MERGED_LABEL,
            DELIVERY_CONFLICT_LABEL,
            DELIVERY_CI_FAILED_LABEL,
        ],
    };
    if repair {
        labels.push(DELIVERY_REPAIR_REQUIRED_LABEL);
    }
    contract_labels(&labels)
}

pub(super) fn supports_repair(response: &NodeResponseContract) -> bool {
    let NodeResponseContract::Verifier { signals, .. } = response else {
        return false;
    };
    signals.values().any(|labels| {
        labels
            .values()
            .iter()
            .any(|label| label.as_str() == DELIVERY_REPAIR_REQUIRED_LABEL)
    })
}

fn contract_field(
    name: &str,
    value_type: PayloadType,
) -> Result<(FieldName, RecordField), ContractValueError> {
    Ok((
        FieldName::new(name)?,
        RecordField {
            value_type,
            required: true,
        },
    ))
}

fn contract_enum(values: &[&str]) -> Result<PayloadType, ContractValueError> {
    Ok(PayloadType::Enum {
        values: contract_labels(values)?,
    })
}

fn contract_labels(values: &[&str]) -> Result<NonEmptyEnumSet, ContractValueError> {
    let values = values
        .iter()
        .copied()
        .map(EnumLabel::new)
        .collect::<Result<Vec<_>, _>>()?;
    NonEmptyEnumSet::new(values)
}
