use std::collections::BTreeMap;

use openengine_cluster_protocol::{
    ContractValueError, EnumLabel, FieldName, NonEmptyEnumSet, PayloadType, RecordField,
};
use serde::Deserialize;
use serde_json::Value;

use crate::native_v2_runner::{NodeResponseContract, NodeRunnerError};

pub const COMMENT_DELIVERY_WORKER_REF: &str = "builtin.comment-delivery.post@1";

fn field(
    name: &str,
    value_type: PayloadType,
    required: bool,
) -> Result<(FieldName, RecordField), ContractValueError> {
    Ok((
        FieldName::new(name)?,
        RecordField {
            value_type,
            required,
        },
    ))
}

pub fn review_scope_schema() -> Result<PayloadType, ContractValueError> {
    Ok(PayloadType::Record {
        fields: [
            "baseRepository",
            "baseBranch",
            "baseRevision",
            "headRepository",
            "headBranch",
            "headRevision",
        ]
        .into_iter()
        .map(|name| field(name, PayloadType::String, false))
        .collect::<Result<_, _>>()?,
    })
}

pub fn comment_input_schema() -> Result<PayloadType, ContractValueError> {
    Ok(PayloadType::Record {
        fields: [
            field("url", PayloadType::String, true)?,
            field("body", PayloadType::String, true)?,
            field("reviewScope", review_scope_schema()?, false)?,
        ]
        .into_iter()
        .collect(),
    })
}

pub fn comment_result_schema() -> Result<PayloadType, ContractValueError> {
    let mut fields = [
        "version",
        "mode",
        "outcome",
        "destinationUrl",
        "repository",
        "discussionKind",
        "discussionId",
        "commentId",
        "url",
        "bodyHash",
    ]
    .into_iter()
    .map(|name| field(name, PayloadType::String, true))
    .collect::<Result<BTreeMap<_, _>, _>>()?;
    fields.insert(
        FieldName::new("sourceRevision")?,
        RecordField {
            value_type: PayloadType::String,
            required: false,
        },
    );
    Ok(PayloadType::Record {
        fields: [field("receipt", PayloadType::Record { fields }, true)?]
            .into_iter()
            .collect(),
    })
}

pub fn comment_signal_labels() -> Result<BTreeMap<FieldName, NonEmptyEnumSet>, ContractValueError> {
    Ok(BTreeMap::from([(
        FieldName::new("delivery")?,
        NonEmptyEnumSet::new(vec![EnumLabel::new("posted")?])?,
    )]))
}

pub fn comment_diagnostic_schema() -> Result<PayloadType, ContractValueError> {
    Ok(PayloadType::Record {
        fields: [field("message", PayloadType::String, true)?]
            .into_iter()
            .collect(),
    })
}

pub fn validate_comment_contract(response: &NodeResponseContract) -> Result<(), NodeRunnerError> {
    let NodeResponseContract::Verifier {
        output,
        signals,
        diagnostic,
    } = response
    else {
        return Err(NodeRunnerError::InvalidRole);
    };
    let matches = output == &comment_result_schema().map_err(|_| NodeRunnerError::Driver)?
        && signals == &comment_signal_labels().map_err(|_| NodeRunnerError::Driver)?
        && diagnostic == &comment_diagnostic_schema().map_err(|_| NodeRunnerError::Driver)?;
    if matches {
        Ok(())
    } else {
        Err(NodeRunnerError::InvalidRole)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct CommentInput {
    pub url: String,
    pub body: String,
    pub review_scope: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommentOutput {
    receipt: Receipt,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Receipt {
    version: String,
    mode: String,
    outcome: String,
    destination_url: String,
    repository: String,
    discussion_kind: String,
    discussion_id: String,
    comment_id: String,
    url: String,
    body_hash: String,
    source_revision: Option<String>,
}

pub fn is_matching_success_receipt(
    output: &Value,
    expected_url: &str,
    source_revision: Option<&str>,
) -> bool {
    let Ok(CommentOutput { receipt }) = serde_json::from_value(output.clone()) else {
        return false;
    };
    let Ok(destination) =
        crate::source_code_provider::adapters::core::comment_destination(expected_url)
    else {
        return false;
    };
    valid_receipt_fields(&receipt)
        && receipt.destination_url == expected_url
        && receipt_matches_destination(&receipt, &destination)
        && receipt.source_revision.as_deref() == source_revision
}

fn valid_receipt_fields(receipt: &Receipt) -> bool {
    receipt.version == "v1"
        && receipt.mode == "comment"
        && receipt.outcome == "posted"
        && super::valid_digest(&receipt.body_hash)
}

fn receipt_matches_destination(
    receipt: &Receipt,
    destination: &crate::source_code_provider::SourceCommentDestination,
) -> bool {
    let kind = match destination.kind {
        crate::source_code_provider::SourceDiscussionKind::Issue => "issue",
        crate::source_code_provider::SourceDiscussionKind::PullRequest => "pull_request",
    };
    receipt.repository == destination.repository
        && receipt.discussion_kind == kind
        && receipt.discussion_id == destination.discussion_id
        && super::valid_id(&receipt.comment_id)
        && super::valid_comment_permalink(destination, &receipt.comment_id, &receipt.url)
}
