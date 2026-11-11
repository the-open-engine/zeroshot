use openengine_cluster_protocol::{
    ContractValueError, EnumLabel, FieldName, NonEmptyEnumSet, PayloadType, RecordField,
};
use serde::Deserialize;
use serde_json::Value;

use crate::native_v2_runner::{NodeResponseContract, NodeRunnerError};

use super::{
    DELIVERY_CI_FAILED_LABEL, DELIVERY_CONFLICT_LABEL, DELIVERY_HEAD_REVISION_FIELD,
    DELIVERY_HEAD_BRANCH_FIELD, DELIVERY_MERGED_LABEL, DELIVERY_MERGE_REVISION_FIELD,
    DELIVERY_MODE_FIELD, DELIVERY_OPENED_LABEL, DELIVERY_OUTCOME_FIELD,
    DELIVERY_PULL_REQUEST_ID_FIELD, DELIVERY_PUSHED_LABEL, DELIVERY_READY_LABEL,
    DELIVERY_REPAIR_REQUIRED_LABEL, DELIVERY_REPOSITORY_FIELD, DELIVERY_SIGNAL_FIELD,
    DELIVERY_TARGET_BRANCH_FIELD, DELIVERY_VERSION_FIELD, DeliveryMode, DeliveryTarget,
    valid_branch, valid_review_id, valid_revision,
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
            Ok(compatible_result_schema(output, mode, repair)?
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
    valid_result_links(result)
        && result.repository == target.repository
        && result.target_branch == target.target_branch
        && valid_revision(&result.head_revision)
        // Admission captures the starting commit, not the remote branch tip. Publishing
        // committed, unpushed work can confirm that exact commit without creating another.
        && receipt_mode_matches(result, mode)
}

fn valid_result_links(result: &DeliveryResultWire) -> bool {
    match result
        .links
        .as_ref()
        .map(|links| (&links.url, &links.commit_url))
    {
        None | Some((None, None)) => true,
        Some((Some(url), Some(commit))) => {
            crate::source_code_provider::same_public_origin(url, commit)
        }
        _ => false,
    }
}

/// Private identity-only evidence using the existing non-success result shape.
pub(crate) fn review_binding_receipt(
    mode: DeliveryMode,
    review: &super::SourceReviewReceipt,
) -> Value {
    super::delivery_result(&super::DeliveryResult {
        mode,
        outcome: DELIVERY_REPAIR_REQUIRED_LABEL,
        review,
        merge_revision: None,
        links: None,
    })
}

/// Recovers review identity, not delivery confirmation, from an owned delivery execution.
pub(crate) fn review_from_receipt(
    output: &Value,
    mode: DeliveryMode,
    target: &DeliveryTarget,
    branch: &str,
) -> Option<super::SourceReviewReceipt> {
    let result: DeliveryResultWire = serde_json::from_value(output.clone()).ok()?;
    if !mode.creates_review()
        || result.version != mode.result_version()
        || result.mode != mode.label()
        || !super::valid_mode_outcome(mode, &result.outcome)
        || result.repository != target.repository
        || result.target_branch != target.target_branch
        || !valid_result_links(&result)
        || !valid_revision(&result.head_revision)
        || !valid_branch(branch)
    {
        return None;
    }
    let review_id = result.pull_request_id.filter(|id| valid_review_id(id))?;
    Some(super::SourceReviewReceipt {
        review_id,
        repository: result.repository,
        target_branch: result.target_branch,
        head_branch: branch.to_owned(),
        head_revision: result.head_revision,
    })
}

fn receipt_mode_matches(result: &DeliveryResultWire, mode: DeliveryMode) -> bool {
    match mode {
        DeliveryMode::Push => {
            result.head_branch.as_deref().is_some_and(valid_branch)
                && result.pull_request_id.is_none()
                && result.merge_revision.is_none()
        }
        DeliveryMode::PullRequest | DeliveryMode::PullRequestV2 | DeliveryMode::MergeV1 => {
            valid_pull_request_receipt(result) && result.merge_revision.is_none()
        }
        DeliveryMode::Merge | DeliveryMode::MergeV3 => {
            valid_pull_request_receipt(result)
                && result.merge_revision.as_deref().is_some_and(valid_revision)
        }
    }
}

fn valid_pull_request_receipt(result: &DeliveryResultWire) -> bool {
    result
        .pull_request_id
        .as_deref()
        .is_some_and(valid_review_id)
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
    head_branch: Option<String>,
    pull_request_id: Option<String>,
    merge_revision: Option<String>,
    links: Option<DeliveryLinksWire>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeliveryLinksWire {
    url: Option<String>,
    commit_url: Option<String>,
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

fn compatible_result_schema(
    output: &PayloadType,
    mode: DeliveryMode,
    repair: bool,
) -> Result<bool, ContractValueError> {
    let mut expected = result_schema(mode, repair)?;
    if output == &expected {
        return Ok(true);
    }
    if let PayloadType::Record { fields } = &mut expected {
        fields.retain(|name, _| name.as_str() != "links");
    }
    Ok(output == &expected)
}

pub(super) fn supports_links(response: &NodeResponseContract) -> bool {
    let NodeResponseContract::Verifier {
        output: PayloadType::Record { fields },
        ..
    } = response
    else {
        return false;
    };
    fields.keys().any(|name| name.as_str() == "links")
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
    ];
    if mode == DeliveryMode::Push {
        fields.push(contract_field(
            DELIVERY_HEAD_BRANCH_FIELD,
            PayloadType::String,
        )?);
    } else {
        fields.push(contract_field(
            DELIVERY_PULL_REQUEST_ID_FIELD,
            PayloadType::String,
        )?);
    }
    if mode != DeliveryMode::Push {
        fields.push(contract_field("links", review_links_schema()?)?);
    }
    if mode.includes_merge_revision() {
        fields.push(contract_field(
            DELIVERY_MERGE_REVISION_FIELD,
            PayloadType::String,
        )?);
    }
    Ok(fields)
}

fn review_links_schema() -> Result<PayloadType, ContractValueError> {
    let fields = ["url", "commitUrl"]
        .into_iter()
        .map(|name| {
            Ok((
                FieldName::new(name)?,
                RecordField {
                    value_type: PayloadType::String,
                    required: false,
                },
            ))
        })
        .collect::<Result<_, ContractValueError>>()?;
    Ok(PayloadType::Record { fields })
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
        DeliveryMode::Push => vec![DELIVERY_PUSHED_LABEL],
        DeliveryMode::PullRequest => vec![DELIVERY_OPENED_LABEL],
        DeliveryMode::PullRequestV2 => vec![
            DELIVERY_READY_LABEL,
            DELIVERY_CONFLICT_LABEL,
            DELIVERY_CI_FAILED_LABEL,
        ],
        DeliveryMode::MergeV1 | DeliveryMode::Merge | DeliveryMode::MergeV3 => vec![
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
