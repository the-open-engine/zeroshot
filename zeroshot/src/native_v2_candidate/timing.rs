//! Client-side timing for deterministic native node transports.
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use serde_json::{Value, json};
use crate::native_v2_runner::{DriverControl, LiveOutput, LiveOutputStream, NodeRunnerError};

pub(super) struct ExecutionTiming {
    started: Instant,
    start_unix_ms: f64,
}

impl ExecutionTiming {
    pub(super) fn start() -> Result<Self, NodeRunnerError> {
        let start_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| {
                NodeRunnerError::DriverDetail("system clock is before Unix epoch".to_owned())
            })?
            .as_secs_f64()
            * 1000.0;
        Ok(Self {
            started: Instant::now(),
            start_unix_ms,
        })
    }

    pub(super) fn finish(self, event: &str, succeeded: bool) -> Value {
        json!({"event": event, "start_unix_ms": self.start_unix_ms,
            "elapsed_ms": self.started.elapsed().as_secs_f64() * 1000.0,
            "succeeded": succeeded})
    }
}

pub(super) async fn publish(control: &DriverControl, value: Value) -> Result<(), NodeRunnerError> {
    control
        .emit_final(LiveOutput::new(
            LiveOutputStream::System,
            value.to_string(),
        )?)
        .await
}
