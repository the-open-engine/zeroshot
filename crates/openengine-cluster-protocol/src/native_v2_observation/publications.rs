//! Fit optional steering summaries beside a published receipt in the final wire projection.
use crate::{HostedRunStatus, HostedRunStatusResult, RunForceResult, RunStatus, RunStatusResult};
use serde::Serialize;
use std::io::{self, Write};

fn published(status: &RunStatus) -> bool {
    matches!(status, RunStatus::Finished { metadata, .. } if !metadata.artifacts.is_empty())
}

macro_rules! fit_published_steering {
    ($projection:ty, $published:expr) => {
        impl $projection {
            /// Keep the newest steering summaries that fit beside the full final receipt.
            /// The vector record retains every steer, including those counted in omittedSteers.
            pub fn fit_published_steering(&mut self) -> serde_json::Result<()> {
                if !self.steering.steers.is_empty() && ($published)(&self.status) {
                    let bytes = json_bytes(self)?;
                    self.steering.fit_response(bytes)?;
                }
                Ok(())
            }
        }
    };
}

fit_published_steering!(RunStatusResult, published);
fit_published_steering!(RunForceResult, published);
fit_published_steering!(HostedRunStatusResult, |status: &HostedRunStatus| status
    .as_target()
    .is_some_and(published));

impl crate::RunSteeringStatus {
    fn fit_response(&mut self, mut bytes: usize) -> serde_json::Result<()> {
        let mut remove = 0;
        let mut omitted = self.omitted_steers;
        while bytes > crate::MAX_REQUEST_RESPONSE_BYTES && remove < self.steers.len() {
            bytes -= json_bytes(&self.steers[remove])? + omitted_field_bytes(omitted);
            remove += 1;
            // A nonempty flattened array has separators; an empty one is omitted entirely.
            bytes -= if remove == self.steers.len() {
                br#","steers":[]"#.len()
            } else {
                1
            };
            omitted = omitted.saturating_add(1);
            bytes += omitted_field_bytes(omitted);
        }
        self.steers.drain(..remove);
        self.omitted_steers = omitted;
        Ok(())
    }
}

fn omitted_field_bytes(count: u64) -> usize {
    count.checked_ilog10().map_or(0, |digits| {
        br#","omittedSteers":"#.len() + digits as usize + 1
    })
}

#[derive(Default)]
struct ByteCount(usize);

impl Write for ByteCount {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn json_bytes(value: &impl Serialize) -> serde_json::Result<usize> {
    let mut count = ByteCount::default();
    serde_json::to_writer(&mut count, value)?;
    Ok(count.0)
}
