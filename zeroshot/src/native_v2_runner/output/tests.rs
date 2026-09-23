use super::*;

fn closed_output_with_terminal_event() -> DurableOutput {
    let (sender, mut output) = durable_event_channel_with_capacity(1);
    assert!(output.try_recv().is_none());
    assert!(
        sender
            .cancelled_terminal
            .try_send(DurableNodeEvent::TokenUsage(None))
            .is_ok()
    );
    drop(sender);
    output
}

#[tokio::test]
async fn closure_recheck_drains_terminal_event_enqueued_after_initial_probe() {
    let mut output = closed_output_with_terminal_event();

    assert_eq!(
        output.recv_pending().await,
        Some(DurableNodeEvent::TokenUsage(None))
    );
    assert_eq!(output.recv().await, Err(AttachReceiveError::Closed));
}

#[tokio::test]
async fn closed_result_rechecks_the_other_lane_after_a_closed_lane_wakes_receive() {
    let mut output = closed_output_with_terminal_event();

    assert_eq!(
        output.closed_result(),
        Some(Ok(DurableNodeEvent::TokenUsage(None)))
    );
    assert_eq!(
        output.closed_result(),
        Some(Err(AttachReceiveError::Closed))
    );
}

#[tokio::test]
async fn overflow_does_not_duplicate_an_existing_incomplete_marker() {
    let (sender, mut output) = durable_event_channel_with_capacity(1);
    sender
        .cancelled_terminal_overflowed
        .store(true, Ordering::Release);
    assert!(
        sender
            .cancelled_terminal
            .try_send(DurableNodeEvent::TokenUsage(None))
            .is_ok()
    );
    drop(sender);

    assert_eq!(output.recv().await, Ok(DurableNodeEvent::TokenUsage(None)));
    assert_eq!(output.recv().await, Err(AttachReceiveError::Closed));
}

#[tokio::test]
async fn coverage_contract_attach_and_durable_batch_closure_are_explicit() {
    assert_eq!(
        closed_live_attach().recv().await,
        Err(AttachReceiveError::Closed)
    );

    let (sender, mut output) = durable_event_channel_with_capacity(1);
    let (_cancel_sender, cancellation) = watch::channel(false);
    sender
        .send(DurableNodeEvent::TokenUsage(None), cancellation)
        .await
        .expect("open durable output");
    output.wait_until_saturated().await;
    let mut events = Vec::new();
    assert_eq!(output.recv_many(&mut events, 0).await, 0);
    assert_eq!(output.recv_many(&mut events, 4).await, 1);
    assert_eq!(events, [DurableNodeEvent::TokenUsage(None)]);
    drop(sender);
    assert_eq!(output.recv_many(&mut events, 4).await, 0);
}

#[tokio::test]
async fn coverage_contract_receive_selects_the_only_open_lane() {
    async fn receive_from_lane(terminal: bool) -> DurableNodeEvent {
        let (events, receiver) = mpsc::channel(1);
        let (cancelled, cancelled_receiver) = mpsc::channel(1);
        let mut output = DurableOutput::new(
            receiver,
            cancelled_receiver,
            Arc::new(AtomicBool::new(false)),
            Arc::new(Notify::new()),
        );
        if terminal {
            drop(events);
            cancelled
                .send(DurableNodeEvent::TokenUsage(None))
                .await
                .expect("terminal lane open");
        } else {
            drop(cancelled);
            events
                .send(DurableNodeEvent::TokenUsage(None))
                .await
                .expect("ordinary lane open");
        }
        output.recv_pending().await.expect("queued event")
    }

    assert_eq!(
        receive_from_lane(false).await,
        DurableNodeEvent::TokenUsage(None)
    );
    assert_eq!(
        receive_from_lane(true).await,
        DurableNodeEvent::TokenUsage(None)
    );
}

#[test]
fn coverage_contract_cancelled_terminal_lane_reports_closed_and_overflow() {
    let (sender, output) = durable_event_channel_with_capacity(1);
    drop(output);
    assert_eq!(
        sender.try_send_cancelled_terminal(DurableNodeEvent::TokenUsage(None)),
        Err(NodeRunnerError::DurableOutputClosed)
    );

    let (sender, _output) = durable_event_channel_with_capacity(1);
    for _ in 0..CANCELLED_TERMINAL_CAPACITY {
        sender
            .cancelled_terminal
            .try_send(DurableNodeEvent::TokenUsage(None))
            .expect("terminal lane capacity matches the documented bound");
    }
    assert!(matches!(
        sender.try_send_cancelled_terminal(DurableNodeEvent::TokenUsage(None)),
        Err(NodeRunnerError::DriverDetail(detail))
            if detail == CANCELLED_TERMINAL_OVERFLOW_DETAIL
    ));
    assert!(sender.cancelled_terminal_overflowed.load(Ordering::Acquire));
}
