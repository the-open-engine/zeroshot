use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

type SessionFixture = (
    ProcessSession,
    mpsc::Receiver<WriterCommand>,
    mpsc::Sender<ProcessOutputChunk>,
    watch::Receiver<bool>,
    watch::Sender<Option<Arc<ProcessSessionOutput>>>,
);

#[test]
fn frame_validation_covers_all_bounds_without_allocating_at_the_limits() {
    assert!(validate_frame_lengths(0, 0).is_ok());
    assert!(validate_frame_lengths(MAX_PROCESS_FRAME_BYTES, MAX_PROCESS_MESSAGE_BYTES).is_ok());
    assert!(
        validate_frame_lengths(0, 1)
            .assert_error()
            .to_string()
            .contains("message length exceeds frame length")
    );
    assert!(
        validate_frame_lengths(MAX_PROCESS_MESSAGE_BYTES + 1, MAX_PROCESS_MESSAGE_BYTES + 1)
            .assert_error()
            .to_string()
            .contains("process message")
    );
    assert!(
        validate_frame_lengths(MAX_PROCESS_FRAMING_OVERHEAD_BYTES + 1, 0)
            .assert_error()
            .to_string()
            .contains("framing overhead")
    );
    assert_eq!(
        ProcessFrame::with_framing(b"headbody".to_vec(), 4)
            .assert_value()
            .into_inner(),
        b"headbody"
    );
}

#[tokio::test]
async fn session_short_circuits_closed_and_completed_input_and_detaches_stdout_once() {
    let (mut closed, _stdin, _stdout, _release, _completion) = session_fixture(None);
    closed.stdin_closed = true;
    assert!(
        closed
            .send(ProcessFrame::new(b"input".to_vec()).assert_value())
            .await
            .assert_error()
            .to_string()
            .contains("stdin is already closed")
    );
    closed.close_stdin().await.assert_value();

    let completed_output = output();
    let (mut completed, _stdin, _stdout, _release, _completion) =
        session_fixture(Some(completed_output.clone()));
    assert!(
        completed
            .send(ProcessFrame::new(b"input".to_vec()).assert_value())
            .await
            .assert_error()
            .to_string()
            .contains("session is no longer running")
    );
    completed.close_stdin().await.assert_value();
    assert_eq!(completed.wait().await.assert_value(), completed_output);

    let (mut session, _stdin, stdout, _release, _completion) = session_fixture(None);
    stdout
        .send(ProcessOutputChunk::from_bytes(b"visible".to_vec()))
        .await
        .assert_value();
    drop(stdout);
    let mut detached = session.detach_stdout();
    assert_eq!(detached.recv().await.assert_value().as_slice(), b"visible");
    assert!(detached.recv().await.is_none());
    assert!(session.recv_stdout().await.is_none());
}

#[tokio::test]
async fn session_reports_missing_writer_and_supervisor_and_release_is_observable() {
    let (session, stdin, _stdout, _release, _completion) = session_fixture(None);
    drop(stdin);
    assert!(
        session
            .send(ProcessFrame::new(b"input".to_vec()).assert_value())
            .await
            .assert_error()
            .to_string()
            .contains("stdin is no longer available")
    );

    let (mut session, stdin, _stdout, _release, _completion) = session_fixture(None);
    drop(stdin);
    assert!(
        session
            .close_stdin()
            .await
            .assert_error()
            .to_string()
            .contains("stdin is no longer available")
    );

    let (session, mut stdin, _stdout, _release, _completion) = session_fixture(None);
    let discard_acknowledgement = tokio::spawn(async move {
        let Some(WriterCommand::Frame(_, acknowledge)) = stdin.recv().await else {
            panic!("expected one input frame");
        };
        drop(acknowledge);
    });
    assert!(
        session
            .send(ProcessFrame::new(b"input".to_vec()).assert_value())
            .await
            .assert_error()
            .to_string()
            .contains("stdin writer stopped")
    );
    discard_acknowledgement.await.assert_value();

    let (completion, mut completion_rx) = watch::channel(None);
    drop(completion);
    assert!(
        await_completion(&mut completion_rx)
            .await
            .assert_error()
            .to_string()
            .contains("supervisor stopped unexpectedly")
    );

    let expected = output();
    let (mut session, _stdin, _stdout, mut release, _completion) =
        session_fixture(Some(expected.clone()));
    assert_eq!(session.release().await.assert_value(), expected);
    release.changed().await.assert_value();
    assert!(*release.borrow());
}

fn session_fixture(completion: Option<ProcessSessionOutput>) -> SessionFixture {
    let (stdout, stdout_rx) = mpsc::channel(2);
    let (stdin, stdin_rx) = mpsc::channel(2);
    let (release, release_rx) = watch::channel(false);
    let (completion_tx, completion_rx) = watch::channel(completion.map(Arc::new));
    (
        ProcessSession {
            stdout: stdout_rx,
            stdin,
            release,
            completion: completion_rx,
            stdin_closed: false,
        },
        stdin_rx,
        stdout,
        release_rx,
        completion_tx,
    )
}

fn output() -> ProcessSessionOutput {
    ProcessSessionOutput {
        launch_evidence: ProcessLaunchEvidence::MayHaveStarted,
        exit_code: Some(0),
        termination_signal: None,
        core_dumped: false,
        stderr_tail: Vec::new(),
        stderr_tail_truncated: false,
        cancelled: false,
        timed_out: false,
        cleanup: ProcessCleanupEvidence::Reaped,
        post_launch_error: None,
    }
}
