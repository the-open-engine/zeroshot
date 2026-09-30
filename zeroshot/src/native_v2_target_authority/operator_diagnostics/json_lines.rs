use std::io::{self, Write};
use std::time::Duration;

use openengine_cluster_protocol::TargetOperatorDiagnostic;
use serde::Serialize;
use tokio::sync::{broadcast, oneshot};

use super::output::DIAGNOSTIC_OUTPUT_CAPACITY;

/// Portable, private JSON-lines output on a dedicated thread.
///
/// Normal lines contain `type: "diagnostic"` and the existing diagnostic fields. Overflow emits
/// `type: "lagged"` with `dropped`. A final `type: "stopped"` line reports whether every producer
/// closed before draining. The writer flushes each line and never runs on an engine task.
/// Output failure drops this collector without affecting execution; a safe message goes to stderr.
/// This adapter adds no persistence, deployment metadata, or network transport.
///
/// Stop producers before calling [`Self::finish`]. Finish drains the bounded queue and flushes;
/// it waits at most two seconds, including when the supplied writer stalls. Abrupt termination
/// can lose unwritten records. Dropping the adapter requests the same drain without waiting.
/// A timed-out writer thread cannot be cancelled and can retain its output until it unblocks or
/// the process exits; it does not delay Tokio runtime shutdown.
#[must_use]
pub struct OperatorDiagnosticJsonLines {
    stop: Option<oneshot::Sender<()>>,
    finished: oneshot::Receiver<io::Result<()>>,
}

impl OperatorDiagnosticJsonLines {
    pub fn start(
        receiver: broadcast::Receiver<TargetOperatorDiagnostic>,
        writer: impl Write + Send + 'static,
    ) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread().build()?;
        let (stop, stopped) = oneshot::channel();
        let (done, finished) = oneshot::channel();
        std::thread::Builder::new()
            .name("operator-diagnostic-output".to_owned())
            .spawn(move || {
                let result = runtime.block_on(collect(receiver, writer, stopped));
                if result.is_err() {
                    let _ = writeln!(io::stderr(), "operator diagnostic JSON output failed");
                }
                let _ = done.send(result);
            })?;
        Ok(Self {
            stop: Some(stop),
            finished,
        })
    }

    pub async fn finish(mut self) -> io::Result<()> {
        self.request_stop();
        tokio::time::timeout(Duration::from_secs(2), &mut self.finished)
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "diagnostic output drain timed out")
            })?
            .map_err(|_| io::Error::other("diagnostic output worker stopped"))?
    }

    fn request_stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

impl Drop for OperatorDiagnosticJsonLines {
    fn drop(&mut self) {
        self.request_stop();
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Line<'a> {
    Diagnostic {
        #[serde(flatten)]
        record: &'a TargetOperatorDiagnostic,
    },
    Lagged {
        dropped: u64,
    },
    Stopped {
        complete: bool,
    },
}

async fn collect(
    mut receiver: broadcast::Receiver<TargetOperatorDiagnostic>,
    mut writer: impl Write,
    mut stopped: oneshot::Receiver<()>,
) -> io::Result<()> {
    loop {
        tokio::select! {
            biased;
            _ = &mut stopped => return drain(&mut receiver, &mut writer),
            received = receiver.recv() => match received {
                Ok(record) => write_line(&mut writer, &Line::Diagnostic { record: &record })?,
                Err(broadcast::error::RecvError::Lagged(dropped)) =>
                    write_line(&mut writer, &Line::Lagged { dropped })?,
                Err(broadcast::error::RecvError::Closed) =>
                    return write_line(&mut writer, &Line::Stopped { complete: true }),
            }
        }
    }
}

fn drain(
    receiver: &mut broadcast::Receiver<TargetOperatorDiagnostic>,
    writer: &mut impl Write,
) -> io::Result<()> {
    // Bound shutdown work even if a caller has not stopped every producer.
    for _ in 0..=DIAGNOSTIC_OUTPUT_CAPACITY {
        match receiver.try_recv() {
            Ok(record) => write_line(writer, &Line::Diagnostic { record: &record })?,
            Err(broadcast::error::TryRecvError::Lagged(dropped)) => {
                write_line(writer, &Line::Lagged { dropped })?
            }
            Err(broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed) => {
                break;
            }
        }
    }
    let complete = receiver.sender_strong_count() == 0 && receiver.is_empty();
    write_line(writer, &Line::Stopped { complete })
}

fn write_line(writer: &mut impl Write, line: &Line<'_>) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, line)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

#[cfg(test)]
#[path = "json_lines/tests.rs"]
mod tests;
