use openengine_cluster_protocol::{
    canonical_value_bytes, ApplyResult, CompiledGraphIr, GraphSpec, Labels, LogLevel, StopMode,
    StopResult, UpdateResult,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::identity::{
    AbsoluteDeadline, ExecutionId, IdempotencyId, LedgerGeneration, LedgerRunId, NodeInstanceId,
    Position, ResourceId,
};
use super::{AdmissionReceipt, DispatchReceipt, MutationReceipt, SettlementReceipt};

pub const RECORD_VERSION_V1: u16 = 1;
pub const MAX_RECORD_PAYLOAD_BYTES: usize = 1024 * 1024;
pub const MAX_APPEND_RECORDS: usize = 1024;
pub const MAX_APPEND_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RANGE_RECORDS: usize = 4096;
pub const MAX_DISCOVERY_RESOURCES: usize = 1024;
const RECORD_HASH_DOMAIN: &[u8] = b"openengine.cluster-ledger.record.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordFamily {
    Control,
    VerifiedIo,
}

impl RecordFamily {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::VerifiedIo => "verified_io",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Admission,
    Dispatch,
    Settlement,
    Void,
    SafeFault,
    EffectIntent,
    EffectReceipt,
    LifecycleUpdate,
    StopRequested,
    Terminal,
    CleanupReceipt,
    MutationReceipt,
}

impl RecordKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admission => "admission",
            Self::Dispatch => "dispatch",
            Self::Settlement => "settlement",
            Self::Void => "void",
            Self::SafeFault => "safe_fault",
            Self::EffectIntent => "effect_intent",
            Self::EffectReceipt => "effect_receipt",
            Self::LifecycleUpdate => "lifecycle_update",
            Self::StopRequested => "stop_requested",
            Self::Terminal => "terminal",
            Self::CleanupReceipt => "cleanup_receipt",
            Self::MutationReceipt => "mutation_receipt",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmissionManifest {
    pub graph_digest: String,
    pub input_digest: String,
    pub policy_digest: String,
    pub catalog_digest: String,
    pub profile_digest: String,
    pub deadline: AbsoluteDeadline,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplyMutationReceipt {
    pub result: ApplyResult,
    pub at_position: Position,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateMutationReceipt {
    pub result: UpdateResult,
    pub at_position: Position,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StopMutationReceipt {
    pub result: StopResult,
    pub at_position: Position,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MutationKind {
    Admit,
    Apply,
    Dispatch,
    Settle,
    SafeFault,
    EffectIntent,
    EffectReceipt,
    Update,
    Stop,
    Terminalize,
    Cleanup,
}

impl MutationKind {
    #[must_use]
    pub(crate) const fn method(self) -> &'static str {
        match self {
            Self::Admit => "admit",
            Self::Apply => "apply",
            Self::Dispatch => "dispatch",
            Self::Settle => "settle",
            Self::SafeFault => "safe_fault",
            Self::EffectIntent => "effect_intent",
            Self::EffectReceipt => "effect_receipt",
            Self::Update => "update",
            Self::Stop => "stop",
            Self::Terminalize => "terminalize",
            Self::Cleanup => "cleanup",
        }
    }

    pub(crate) fn close(self, value: &[u8]) -> Result<ClosedMutationReceipt, RecordError> {
        fn decode<T: for<'de> Deserialize<'de>>(value: &[u8]) -> Result<T, RecordError> {
            serde_json::from_slice(value).map_err(|_| RecordError::InvalidPayload)
        }

        Ok(match self {
            Self::Admit => ClosedMutationReceipt::Admit(decode(value)?),
            Self::Apply => ClosedMutationReceipt::Apply(decode(value)?),
            Self::Dispatch => ClosedMutationReceipt::Dispatch(decode(value)?),
            Self::Settle => ClosedMutationReceipt::Settle(decode(value)?),
            Self::SafeFault => ClosedMutationReceipt::SafeFault(decode(value)?),
            Self::EffectIntent => ClosedMutationReceipt::EffectIntent(decode(value)?),
            Self::EffectReceipt => ClosedMutationReceipt::EffectReceipt(decode(value)?),
            Self::Update => ClosedMutationReceipt::Update(decode(value)?),
            Self::Stop => ClosedMutationReceipt::Stop(decode(value)?),
            Self::Terminalize => ClosedMutationReceipt::Terminalize(decode(value)?),
            Self::Cleanup => ClosedMutationReceipt::Cleanup(decode(value)?),
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(
    tag = "method",
    content = "receipt",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ClosedMutationReceipt {
    Admit(AdmissionReceipt),
    Apply(ApplyMutationReceipt),
    Dispatch(DispatchReceipt),
    Settle(SettlementReceipt),
    SafeFault(MutationReceipt),
    EffectIntent(MutationReceipt),
    EffectReceipt(MutationReceipt),
    Update(UpdateMutationReceipt),
    Stop(StopMutationReceipt),
    Terminalize(MutationReceipt),
    Cleanup(MutationReceipt),
}

impl ClosedMutationReceipt {
    #[must_use]
    pub(crate) const fn kind(&self) -> MutationKind {
        match self {
            Self::Admit(_) => MutationKind::Admit,
            Self::Apply(_) => MutationKind::Apply,
            Self::Dispatch(_) => MutationKind::Dispatch,
            Self::Settle(_) => MutationKind::Settle,
            Self::SafeFault(_) => MutationKind::SafeFault,
            Self::EffectIntent(_) => MutationKind::EffectIntent,
            Self::EffectReceipt(_) => MutationKind::EffectReceipt,
            Self::Update(_) => MutationKind::Update,
            Self::Stop(_) => MutationKind::Stop,
            Self::Terminalize(_) => MutationKind::Terminalize,
            Self::Cleanup(_) => MutationKind::Cleanup,
        }
    }

    #[must_use]
    pub const fn at_position(&self) -> Position {
        match self {
            Self::Admit(receipt) => receipt.at_position,
            Self::Apply(receipt) => receipt.at_position,
            Self::Dispatch(receipt) => receipt.at_position,
            Self::Settle(receipt) => receipt.at_position,
            Self::SafeFault(receipt)
            | Self::EffectIntent(receipt)
            | Self::EffectReceipt(receipt)
            | Self::Terminalize(receipt)
            | Self::Cleanup(receipt) => receipt.at_position,
            Self::Update(receipt) => receipt.at_position,
            Self::Stop(receipt) => receipt.at_position,
        }
    }

    pub fn encode_value(&self) -> Result<Vec<u8>, RecordError> {
        let value = match self {
            Self::Admit(value) => serde_json::to_value(value),
            Self::Apply(value) => serde_json::to_value(value),
            Self::Dispatch(value) => serde_json::to_value(value),
            Self::Settle(value) => serde_json::to_value(value),
            Self::SafeFault(value)
            | Self::EffectIntent(value)
            | Self::EffectReceipt(value)
            | Self::Terminalize(value)
            | Self::Cleanup(value) => serde_json::to_value(value),
            Self::Update(value) => serde_json::to_value(value),
            Self::Stop(value) => serde_json::to_value(value),
        }
        .map_err(|_| RecordError::Encoding)?;
        canonical_value_bytes(&value).map_err(|_| RecordError::Encoding)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecordPayload {
    Admission {
        generation: LedgerGeneration,
        run_id: LedgerRunId,
        graph: Box<GraphSpec>,
        compiled_ir: Box<CompiledGraphIr>,
        input: Value,
        manifest: AdmissionManifest,
    },
    Dispatch {
        node_instance_id: NodeInstanceId,
        execution_id: ExecutionId,
        turn_id: String,
    },
    Settlement {
        execution_id: ExecutionId,
        output: Value,
    },
    Void {
        execution_id: ExecutionId,
    },
    SafeFault {
        execution_id: Option<ExecutionId>,
        encoded_fault: Vec<u8>,
        consequence: TerminalOutcome,
    },
    EffectIntent {
        execution_id: ExecutionId,
        effect_id: String,
        request_digest: String,
    },
    EffectReceipt {
        effect_id: String,
        reconciliation_digest: String,
    },
    LifecycleUpdate {
        labels: Option<Labels>,
        log_level: Option<LogLevel>,
        suspended: Option<bool>,
    },
    StopRequested {
        accepted_mode: StopMode,
        effective_mode: StopMode,
    },
    Terminal {
        outcome: TerminalOutcome,
    },
    CleanupReceipt {
        resource_id: String,
        reconciliation_digest: String,
    },
    MutationReceipt {
        key: IdempotencyId,
        fingerprint: [u8; 32],
        receipt: ClosedMutationReceipt,
    },
}

impl RecordPayload {
    #[must_use]
    pub const fn family(&self) -> RecordFamily {
        match self {
            Self::Settlement { .. } => RecordFamily::VerifiedIo,
            Self::Admission { .. }
            | Self::Dispatch { .. }
            | Self::Void { .. }
            | Self::SafeFault { .. }
            | Self::EffectIntent { .. }
            | Self::EffectReceipt { .. }
            | Self::LifecycleUpdate { .. }
            | Self::StopRequested { .. }
            | Self::Terminal { .. }
            | Self::CleanupReceipt { .. }
            | Self::MutationReceipt { .. } => RecordFamily::Control,
        }
    }

    #[must_use]
    pub const fn record_kind(&self) -> RecordKind {
        match self {
            Self::Admission { .. } => RecordKind::Admission,
            Self::Dispatch { .. } => RecordKind::Dispatch,
            Self::Settlement { .. } => RecordKind::Settlement,
            Self::Void { .. } => RecordKind::Void,
            Self::SafeFault { .. } => RecordKind::SafeFault,
            Self::EffectIntent { .. } => RecordKind::EffectIntent,
            Self::EffectReceipt { .. } => RecordKind::EffectReceipt,
            Self::LifecycleUpdate { .. } => RecordKind::LifecycleUpdate,
            Self::StopRequested { .. } => RecordKind::StopRequested,
            Self::Terminal { .. } => RecordKind::Terminal,
            Self::CleanupReceipt { .. } => RecordKind::CleanupReceipt,
            Self::MutationReceipt { .. } => RecordKind::MutationReceipt,
        }
    }

    pub fn encode_canonical(&self) -> Result<Vec<u8>, RecordError> {
        let value = serde_json::to_value(self).map_err(|_| RecordError::Encoding)?;
        let encoded = canonical_value_bytes(&value).map_err(|_| RecordError::Encoding)?;
        if encoded.len() > MAX_RECORD_PAYLOAD_BYTES {
            return Err(RecordError::PayloadTooLarge);
        }
        Ok(encoded)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Succeeded,
    Failed,
    Stopped,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LedgerRecord {
    pub resource_id: ResourceId,
    pub sequence: Position,
    pub family: RecordFamily,
    pub kind: RecordKind,
    pub version: u16,
    pub payload: Vec<u8>,
    pub previous_hash: [u8; 32],
    pub record_hash: [u8; 32],
}

impl LedgerRecord {
    pub fn new(
        resource_id: ResourceId,
        sequence: Position,
        payload: &RecordPayload,
        previous_hash: [u8; 32],
    ) -> Result<Self, RecordError> {
        if sequence == Position::ZERO {
            return Err(RecordError::InvalidSequence);
        }
        let encoded = payload.encode_canonical()?;
        let family = payload.family();
        let kind = payload.record_kind();
        let record_hash = calculate_record_hash(
            &resource_id,
            sequence,
            family,
            kind,
            RECORD_VERSION_V1,
            &encoded,
            previous_hash,
        );
        Ok(Self {
            resource_id,
            sequence,
            family,
            kind,
            version: RECORD_VERSION_V1,
            payload: encoded,
            previous_hash,
            record_hash,
        })
    }

    pub fn decode_payload(&self) -> Result<RecordPayload, RecordError> {
        self.validate_integrity()?;
        let payload: RecordPayload =
            serde_json::from_slice(&self.payload).map_err(|_| RecordError::InvalidPayload)?;
        if payload.family() != self.family || payload.record_kind() != self.kind {
            return Err(RecordError::KindMismatch);
        }
        if payload.encode_canonical()? != self.payload {
            return Err(RecordError::NonCanonicalPayload);
        }
        Ok(payload)
    }

    pub fn validate_integrity(&self) -> Result<(), RecordError> {
        if self.version != RECORD_VERSION_V1 {
            return Err(RecordError::UnknownVersion(self.version));
        }
        if self.sequence == Position::ZERO {
            return Err(RecordError::InvalidSequence);
        }
        if self.payload.len() > MAX_RECORD_PAYLOAD_BYTES {
            return Err(RecordError::PayloadTooLarge);
        }
        let expected = calculate_record_hash(
            &self.resource_id,
            self.sequence,
            self.family,
            self.kind,
            self.version,
            &self.payload,
            self.previous_hash,
        );
        if expected != self.record_hash {
            return Err(RecordError::HashMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RecordError {
    #[error("record payload exceeds 1 MiB")]
    PayloadTooLarge,
    #[error("record payload encoding failed")]
    Encoding,
    #[error("record payload is invalid")]
    InvalidPayload,
    #[error("record payload is not canonical")]
    NonCanonicalPayload,
    #[error("record kind or family does not match its payload")]
    KindMismatch,
    #[error("unknown ledger record version {0}")]
    UnknownVersion(u16),
    #[error("record sequence must be positive")]
    InvalidSequence,
    #[error("ledger record hash mismatch")]
    HashMismatch,
}

fn calculate_record_hash(
    resource_id: &ResourceId,
    sequence: Position,
    family: RecordFamily,
    kind: RecordKind,
    version: u16,
    payload: &[u8],
    previous_hash: [u8; 32],
) -> [u8; 32] {
    let payload_digest = Sha256::digest(payload);
    let mut hasher = Sha256::new();
    hasher.update(RECORD_HASH_DOMAIN);
    hash_field(&mut hasher, resource_id.as_str().as_bytes());
    hasher.update(sequence.get().to_be_bytes());
    hash_field(&mut hasher, family.as_str().as_bytes());
    hash_field(&mut hasher, kind.as_str().as_bytes());
    hasher.update(version.to_be_bytes());
    hasher.update(payload_digest);
    hasher.update(previous_hash);
    hasher.finalize().into()
}

fn hash_field(hasher: &mut Sha256, field: &[u8]) {
    hasher.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(field);
}
