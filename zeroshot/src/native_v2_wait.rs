//! Fixed host-run wait verifier contract and bounded wall-clock deadline validation.
use std::collections::BTreeMap;
use openengine_cluster_protocol::{
    ContractValueError, EnumLabel, FieldName, NonEmptyEnumSet, PayloadType, RecordField,
    UnixTimestampMillis, WorkerOutcome,
};
use serde_json::{Value, json};

pub const WAIT_WORKER_REF: &str = "builtin.request.wait@1";
pub const MAX_WAIT_MILLIS: u64 = 30 * 24 * 60 * 60 * 1000;

pub fn input_schema() -> Result<PayloadType, ContractValueError> {
    Ok(PayloadType::Record {
        fields: [
            (
                FieldName::new("dueAt")?,
                RecordField {
                    value_type: PayloadType::String,
                    required: false,
                },
            ),
            (
                FieldName::new("durationMs")?,
                RecordField {
                    value_type: PayloadType::Integer,
                    required: false,
                },
            ),
        ]
        .into_iter()
        .collect(),
    })
}

pub fn signal_labels() -> Result<BTreeMap<FieldName, NonEmptyEnumSet>, ContractValueError> {
    Ok(BTreeMap::from([(
        FieldName::new("wait")?,
        NonEmptyEnumSet::new(vec![EnumLabel::new("elapsed")?])?,
    )]))
}

pub fn diagnostic_schema() -> Result<PayloadType, ContractValueError> {
    Ok(PayloadType::Record {
        fields: [(
            FieldName::new("message")?,
            RecordField {
                value_type: PayloadType::String,
                required: true,
            },
        )]
        .into_iter()
        .collect(),
    })
}

pub fn elapsed() -> Result<WorkerOutcome, ContractValueError> {
    Ok(WorkerOutcome::Verifier {
        output: Value::Null,
        signals: BTreeMap::from([(FieldName::new("wait")?, EnumLabel::new("elapsed")?)]),
        diagnostic: json!({"message": "wait elapsed"}),
        artifacts: Vec::new(),
    })
}

/// Tests supply a wall clock independently from Tokio's monotonic scheduling clock.
pub type WallClock = std::sync::Arc<dyn Fn() -> UnixTimestampMillis + Send + Sync>;

pub fn deadline(
    input: &Value,
    now: UnixTimestampMillis,
) -> Result<UnixTimestampMillis, &'static str> {
    let object = input.as_object().ok_or("wait input must be an object")?;
    if object.len() != 1 {
        return Err("wait requires exactly one of dueAt and durationMs");
    }
    let due = if let Some(value) = object.get("dueAt") {
        let text = value
            .as_str()
            .ok_or("dueAt must be an RFC 3339 string with offset")?;
        let date =
            time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
                .map_err(|_| "dueAt must be an RFC 3339 string with offset")?;
        let millis = date.unix_timestamp_nanos() / 1_000_000;
        if millis <= 0 {
            UnixTimestampMillis::MIN
        } else {
            u64::try_from(millis)
                .ok()
                .and_then(|n| UnixTimestampMillis::new(n).ok())
                .ok_or("dueAt is out of range")?
        }
    } else if let Some(value) = object.get("durationMs") {
        let duration = value.as_i64().ok_or("durationMs must be an integer")?;
        let duration = u64::try_from(duration).unwrap_or(0);
        if duration > MAX_WAIT_MILLIS {
            return Err("wait exceeds 30 days");
        }
        now.get()
            .checked_add(duration)
            .and_then(|n| UnixTimestampMillis::new(n).ok())
            .ok_or("wait deadline overflow")?
    } else {
        return Err("wait requires exactly one of dueAt and durationMs");
    };
    if due.get().saturating_sub(now.get()) > MAX_WAIT_MILLIS {
        return Err("wait exceeds 30 days");
    }
    Ok(due)
}

/// Host-only policy. Direct and local runs use the default, with no in-place limit.
#[derive(Clone)]
pub struct WaitPolicy {
    pub in_place_threshold: Option<std::time::Duration>,
    pub wall_clock: WallClock,
}
impl Default for WaitPolicy {
    fn default() -> Self {
        Self {
            in_place_threshold: None,
            wall_clock: std::sync::Arc::new(crate::native_v2_runner::current_timestamp),
        }
    }
}

#[cfg(test)]
mod tests;
