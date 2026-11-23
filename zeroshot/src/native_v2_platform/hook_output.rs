use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::io::{AsyncRead, AsyncReadExt};
use crate::native_v2_platform::{PreparationProgress, CapsuleAllocationUnavailable};
use crate::native_v2_capsule::provider_process::safe_provider_text;

const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_LINE_BYTES: usize = 8 * 1024;

#[derive(Clone)]
pub(crate) struct HookOutput {
    progress: Arc<dyn PreparationProgress>,
    redactions: Arc<Vec<String>>,
    phase: &'static str,
    bytes: Arc<AtomicUsize>,
}

impl HookOutput {
    pub fn new(
        progress: Arc<dyn PreparationProgress>,
        redactions: Vec<String>,
        phase: &'static str,
    ) -> Self {
        Self {
            progress,
            redactions: Arc::new(redactions),
            phase,
            bytes: Arc::new(AtomicUsize::new(0)),
        }
    }

    async fn line(&self, line: &[u8], overflow: bool) -> Result<(), CapsuleAllocationUnavailable> {
        let safe = if overflow {
            "[oversized output line omitted]".to_owned()
        } else {
            safe_provider_text(&String::from_utf8_lossy(line), &self.redactions)
        };
        let safe: String = safe
            .chars()
            .map(|c| if c.is_control() && c != '\t' { ' ' } else { c })
            .collect();
        let line = format!("[{}] {safe}", self.phase);
        let line = crate::native_v2_runner::bounded_log_text(
            line,
            crate::v2_run_ledger::MAX_SAFE_LOG_BYTES,
        );
        let previous = self
            .bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |bytes| {
                Some(bytes.saturating_add(line.len()).min(MAX_OUTPUT_BYTES))
            })
            .unwrap_or_else(|bytes| bytes);
        if previous >= MAX_OUTPUT_BYTES {
            return Ok(());
        }
        if previous.saturating_add(line.len()) >= MAX_OUTPUT_BYTES {
            return self
                .progress
                .log(&format!(
                    "[{}] [output limit reached; remaining output is drained]",
                    self.phase
                ))
                .await;
        }
        self.progress.log(&line).await
    }
}

/// Drain services' inherited pipes for the entire run, even after the hook shell exits.
/// Oversized lines are discarded whole so clipping cannot expose part of a credential.
pub(crate) async fn drain(
    mut pipe: impl AsyncRead + Unpin,
    output: HookOutput,
) -> Result<(), CapsuleAllocationUnavailable> {
    let mut chunk = [0_u8; 4096];
    let mut stream = HookOutputStream::new(output);
    loop {
        let size = match pipe.read(&mut chunk).await {
            Ok(0) => break,
            Err(_) => return Err(CapsuleAllocationUnavailable::Runtime),
            Ok(size) => size,
        };
        stream.write(&chunk[..size]).await?;
    }
    stream.finish().await
}

pub(crate) struct HookOutputStream {
    output: HookOutput,
    line: OutputLine,
}

impl HookOutputStream {
    pub(crate) fn new(output: HookOutput) -> Self {
        Self {
            output,
            line: OutputLine::default(),
        }
    }

    pub(crate) async fn write(&mut self, chunk: &[u8]) -> Result<(), CapsuleAllocationUnavailable> {
        for byte in chunk {
            if self.line.push(*byte) {
                self.output
                    .line(&self.line.bytes, self.line.overflow)
                    .await?;
                self.line = OutputLine::default();
            }
        }
        Ok(())
    }

    pub(crate) async fn finish(self) -> Result<(), CapsuleAllocationUnavailable> {
        if !self.line.bytes.is_empty() {
            self.output
                .line(&self.line.bytes, self.line.overflow)
                .await?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct OutputLine {
    bytes: Vec<u8>,
    overflow: bool,
}
impl OutputLine {
    fn push(&mut self, byte: u8) -> bool {
        if byte == b'\n' {
            return true;
        }
        if self.bytes.len() < MAX_LINE_BYTES {
            self.bytes.push(byte);
        } else {
            self.overflow = true;
        }
        false
    }
}
