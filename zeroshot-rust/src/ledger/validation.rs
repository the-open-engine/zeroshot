use openengine_cluster_protocol::{canonical_value_bytes, CompiledGraphIr, GraphSpec};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::identity::{AbsoluteDeadline, LedgerGeneration, LedgerRunId};
use super::record::AdmissionManifest;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ValidationError;

pub(crate) fn valid_component(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

pub(crate) fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn digest_value<T: Serialize>(value: &T) -> Result<String, ValidationError> {
    let value = serde_json::to_value(value).map_err(|_| ValidationError)?;
    let bytes = canonical_value_bytes(&value).map_err(|_| ValidationError)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn admission_manifest(
    graph: &GraphSpec,
    compiled_ir: &CompiledGraphIr,
    input: &Value,
    deadline: AbsoluteDeadline,
) -> Result<AdmissionManifest, ValidationError> {
    Ok(AdmissionManifest {
        graph_digest: digest_value(graph)?,
        input_digest: digest_value(input)?,
        policy_digest: digest_value(&compiled_ir.policy)?,
        catalog_digest: digest_value(&compiled_ir.root)?,
        profile_digest: digest_value(&compiled_ir.profile)?,
        deadline,
    })
}

pub(crate) fn run_id(
    generation: LedgerGeneration,
    graph_digest: &str,
) -> Result<LedgerRunId, ValidationError> {
    if !valid_digest(graph_digest) {
        return Err(ValidationError);
    }
    LedgerRunId::new(format!("run-{}-{}", generation.get(), &graph_digest[..16]))
        .map_err(|_| ValidationError)
}
