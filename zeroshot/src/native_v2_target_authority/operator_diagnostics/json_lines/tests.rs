use super::*;
use std::sync::{Arc, Condvar, Mutex};
use openengine_cluster_protocol::RunId;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use crate::native_v2_target_authority::{
    NewOperatorDiagnostic, OperatorDiagnosticOutput, OperatorDiagnosticStore,
};

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().assert_value().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Buffer {
    fn lines(&self) -> Vec<serde_json::Value> {
        let bytes = self.0.lock().assert_value().clone();
        let text = String::from_utf8(bytes).assert_value();
        text.lines()
            .map(|line| serde_json::from_str(line).assert_value())
            .collect()
    }
}

fn record(store: &OperatorDiagnosticStore, text: &str) {
    store.record(NewOperatorDiagnostic {
        run_id: RunId::new("run-json"),
        code: "runtime_failed",
        operation: "supervisor.drive",
        exit_status: None,
        stdout: text.to_owned(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
    });
}

async fn collected_lines(
    receiver: broadcast::Receiver<TargetOperatorDiagnostic>,
) -> Vec<serde_json::Value> {
    let buffer = Buffer::default();
    OperatorDiagnosticJsonLines::start(receiver, buffer.clone())
        .assert_value()
        .finish()
        .await
        .assert_value();
    buffer.lines()
}

#[tokio::test]
async fn json_lines_preserve_canonical_fields_and_escape_multiline_text() {
    let (output, receiver) = OperatorDiagnosticOutput::channel();
    let store = OperatorDiagnosticStore::new(Some(output));
    record(&store, "first\nsecond\t\"quoted\"");
    let expected = store
        .snapshot(&RunId::new("run-json"))
        .diagnostics
        .remove(0);
    let buffer = Buffer::default();
    let collector = OperatorDiagnosticJsonLines::start(receiver, buffer.clone()).assert_value();
    drop(store);
    collector.finish().await.assert_value();
    let lines = buffer.lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["type"], "diagnostic");
    let mut fields = lines[0].clone();
    fields.as_object_mut().assert_value().remove("type");
    assert_eq!(fields, serde_json::to_value(expected).assert_value());
    assert_eq!(
        lines[1],
        serde_json::json!({"type":"stopped", "complete":true})
    );
}

#[tokio::test]
async fn overflow_reports_loss_and_drains_the_latest_bounded_records() {
    let (output, receiver) = OperatorDiagnosticOutput::channel();
    let store = OperatorDiagnosticStore::new(Some(output));
    for _ in 0..DIAGNOSTIC_OUTPUT_CAPACITY + 3 {
        record(&store, "retained");
    }
    drop(store);
    let lines = collected_lines(receiver).await;
    assert_eq!(lines.len(), DIAGNOSTIC_OUTPUT_CAPACITY + 2);
    assert_eq!(lines[0], serde_json::json!({"type":"lagged", "dropped":3}));
    assert_eq!(lines[1]["id"], "4");
    assert_eq!(lines[DIAGNOSTIC_OUTPUT_CAPACITY]["id"], "131");
    assert_eq!(lines.last().assert_value()["complete"], true);
}

#[tokio::test]
async fn finish_does_not_wait_for_active_producers_and_marks_the_cutoff_incomplete() {
    let (output, receiver) = OperatorDiagnosticOutput::channel();
    let store = OperatorDiagnosticStore::new(Some(output));
    record(&store, "before cutoff");
    let lines = collected_lines(receiver).await;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines.last().assert_value()["complete"], false);
    record(&store, "after cutoff");
    assert_eq!(store.snapshot(&RunId::new("run-json")).diagnostics.len(), 2);
}

struct BrokenWriter;
impl Write for BrokenWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn broken_output_preserves_the_private_snapshot_and_returns_the_failure() {
    let (output, receiver) = OperatorDiagnosticOutput::channel();
    let store = OperatorDiagnosticStore::new(Some(output));
    record(&store, "preserved");
    let collector = OperatorDiagnosticJsonLines::start(receiver, BrokenWriter).assert_value();
    assert_eq!(
        collector.finish().await.assert_error().kind(),
        io::ErrorKind::BrokenPipe
    );
    record(&store, "still independent");
    assert_eq!(store.snapshot(&RunId::new("run-json")).diagnostics.len(), 2);
}

struct StalledWriter {
    entered: Arc<tokio::sync::Notify>,
    released: Arc<(Mutex<bool>, Condvar)>,
}
impl Write for StalledWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.entered.notify_one();
        let (lock, ready) = &*self.released;
        let mut released = lock.lock().assert_value();
        while !*released {
            released = ready.wait(released).assert_value();
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn a_stalled_writer_does_not_block_async_work_or_shutdown() {
    let (output, receiver) = OperatorDiagnosticOutput::channel();
    let store = OperatorDiagnosticStore::new(Some(output));
    record(&store, "stalled");
    let entered = Arc::new(tokio::sync::Notify::new());
    let released = Arc::new((Mutex::new(false), Condvar::new()));
    let writer = StalledWriter {
        entered: entered.clone(),
        released: released.clone(),
    };
    let collector = OperatorDiagnosticJsonLines::start(receiver, writer).assert_value();
    entered.notified().await;
    assert_eq!(
        collector.finish().await.assert_error().kind(),
        io::ErrorKind::TimedOut
    );
    let (lock, ready) = &*released;
    *lock.lock().assert_value() = true;
    ready.notify_all();
}
