use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot, watch};

use super::*;

struct FailingReader {
    kind: io::ErrorKind,
    message: &'static str,
}

impl AsyncRead for FailingReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        _buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Err(io::Error::new(self.kind, self.message)))
    }
}

#[derive(Clone, Copy)]
enum WriteFailure {
    Write,
    Shutdown,
}

struct FailingWriter(WriteFailure);

impl AsyncWrite for FailingWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.0 {
            WriteFailure::Write => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "injected write cause",
            ))),
            WriteFailure::Shutdown => Poll::Ready(Ok(buffer.len())),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.0 {
            WriteFailure::Write => Poll::Ready(Ok(())),
            WriteFailure::Shutdown => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected shutdown cause",
            ))),
        }
    }
}

#[tokio::test]
async fn stdout_and_stderr_read_failures_retain_the_io_cause() {
    let (output, _output_rx) = mpsc::channel(1);
    let stdout = read_stdout(
        FailingReader {
            kind: io::ErrorKind::ConnectionReset,
            message: "injected stdout cause",
        },
        output,
    )
    .await
    .assert_error()
    .into_detail();
    let stderr = read_stderr(
        FailingReader {
            kind: io::ErrorKind::UnexpectedEof,
            message: "injected stderr cause",
        },
        std::sync::Arc::new(std::sync::Mutex::new(TailBuffer::new(16))),
    )
    .await
    .assert_error()
    .into_detail();

    assert_io_detail(
        &stdout,
        "process stdout read failed",
        "ConnectionReset",
        "injected stdout cause",
    );
    assert_io_detail(
        &stderr,
        "process stderr read failed",
        "UnexpectedEof",
        "injected stderr cause",
    );
}

#[tokio::test]
async fn stdin_write_failure_reaches_acknowledgement_and_supervisor_channel() {
    let (commands, command_rx) = mpsc::channel(1);
    let (_stop, stop_rx) = watch::channel(false);
    let (failures, mut failure_rx) = mpsc::unbounded_channel();
    let (acknowledge, acknowledged) = oneshot::channel();
    commands
        .send(WriterCommand::Frame(b"input".to_vec(), acknowledge))
        .await
        .assert_value();

    run_writer(
        FailingWriter(WriteFailure::Write),
        command_rx,
        stop_rx,
        failures,
    )
    .await;

    let acknowledged = acknowledged.await.assert_value().assert_error();
    let reported = failure_rx.recv().await.assert_value().into_detail();
    assert_io_detail(
        &acknowledged,
        "process stdin write failed",
        "BrokenPipe",
        "injected write cause",
    );
    assert_eq!(reported, acknowledged);
}

#[tokio::test]
async fn explicit_and_channel_drop_shutdown_failures_retain_the_io_cause() {
    let explicit = shutdown_case(true).await;
    let channel_drop = shutdown_case(false).await;

    assert_io_detail(
        &explicit,
        "process stdin shutdown failed",
        "PermissionDenied",
        "injected shutdown cause",
    );
    assert_io_detail(
        &channel_drop,
        "process stdin shutdown after command channel closed failed",
        "PermissionDenied",
        "injected shutdown cause",
    );
}

#[tokio::test]
async fn missing_child_pipes_report_immediate_static_failures() {
    let (output, _output_rx) = mpsc::channel(1);
    let (failures, mut failure_rx) = mpsc::unbounded_channel();
    spawn_stdout_pump(None, output, failures)
        .await
        .assert_value();
    assert_eq!(
        failure_rx.recv().await.assert_value().into_detail(),
        "process stdout pipe is unavailable"
    );

    let tail = std::sync::Arc::new(std::sync::Mutex::new(TailBuffer::new(16)));
    let (failures, mut failure_rx) = mpsc::unbounded_channel();
    spawn_stderr_pump(None, tail, failures).await.assert_value();
    assert_eq!(
        failure_rx.recv().await.assert_value().into_detail(),
        "process stderr pipe is unavailable"
    );

    let (_commands, command_rx) = mpsc::channel(1);
    let (_stop, stop_rx) = watch::channel(false);
    let (failures, mut failure_rx) = mpsc::unbounded_channel();
    spawn_writer(None, command_rx, stop_rx, failures)
        .await
        .assert_value();
    assert_eq!(
        failure_rx.recv().await.assert_value().into_detail(),
        "process stdin pipe is unavailable"
    );
}

#[tokio::test]
async fn ordinary_readers_and_writer_preserve_small_payloads_and_clean_shutdown() {
    let (output, mut output_rx) = mpsc::channel(1);
    read_stdout(&b"stdout"[..], output).await.assert_value();
    assert_eq!(output_rx.recv().await.assert_value().as_slice(), b"stdout");
    assert!(output_rx.recv().await.is_none());

    let tail = std::sync::Arc::new(std::sync::Mutex::new(TailBuffer::new(16)));
    read_stderr(&b"stderr"[..], tail.clone())
        .await
        .assert_value();
    assert_eq!(
        tail.lock().assert_value().snapshot().bytes,
        b"stderr".to_vec()
    );

    let (commands, command_rx) = mpsc::channel(2);
    let (_stop, stop_rx) = watch::channel(false);
    let (failures, mut failure_rx) = mpsc::unbounded_channel();
    let (writer, mut reader) = tokio::io::duplex(32);
    let (frame_ack, frame_acked) = oneshot::channel();
    let (close_ack, close_acked) = oneshot::channel();
    commands
        .send(WriterCommand::Frame(b"input".to_vec(), frame_ack))
        .await
        .assert_value();
    commands
        .send(WriterCommand::Close(close_ack))
        .await
        .assert_value();
    drop(commands);
    run_writer(writer, command_rx, stop_rx, failures).await;
    frame_acked.await.assert_value().assert_value();
    close_acked.await.assert_value().assert_value();
    let mut written = Vec::new();
    reader.read_to_end(&mut written).await.assert_value();
    assert_eq!(written, b"input");
    assert!(failure_rx.try_recv().is_err());
}

#[tokio::test]
async fn writer_stop_and_release_failure_classification_are_deterministic() {
    let (_commands, command_rx) = mpsc::channel(1);
    let (stop, stop_rx) = watch::channel(false);
    let (failures, mut failure_rx) = mpsc::unbounded_channel();
    let writer = tokio::spawn(run_writer(tokio::io::sink(), command_rx, stop_rx, failures));
    stop.send_replace(true);
    writer.await.assert_value();
    assert!(failure_rx.try_recv().is_err());

    let ordinary = IoFailure::static_detail("ordinary");
    assert!(ordinary.should_report(false));
    assert!(ordinary.should_report(true));
    let release = IoFailure::release_artifact("release artifact");
    assert!(release.should_report(false));
    assert!(!release.should_report(true));
    assert_eq!(release.into_detail(), "release artifact");
}

async fn shutdown_case(explicit: bool) -> String {
    let (commands, command_rx) = mpsc::channel(1);
    let (_stop, stop_rx) = watch::channel(false);
    let (failures, mut failure_rx) = mpsc::unbounded_channel();
    let acknowledged = if explicit {
        let (acknowledge, acknowledged) = oneshot::channel();
        commands
            .send(WriterCommand::Close(acknowledge))
            .await
            .assert_value();
        Some(acknowledged)
    } else {
        None
    };
    drop(commands);

    run_writer(
        FailingWriter(WriteFailure::Shutdown),
        command_rx,
        stop_rx,
        failures,
    )
    .await;

    let reported = failure_rx.recv().await.assert_value().into_detail();
    if let Some(acknowledged) = acknowledged {
        assert_eq!(acknowledged.await.assert_value().assert_error(), reported);
    }
    reported
}

fn assert_io_detail(detail: &str, operation: &str, kind: &str, message: &str) {
    assert!(detail.starts_with(operation));
    assert!(detail.contains(&format!("kind={kind}")));
    assert!(detail.contains("raw_os_error=none"));
    assert!(detail.contains(&format!("message={message}")));
}
