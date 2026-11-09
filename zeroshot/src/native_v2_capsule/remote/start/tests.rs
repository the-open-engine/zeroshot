use super::*;

#[tokio::test]
async fn failed_close_wins_when_loss_arrives_after_its_waiter_was_polled() {
    let (sender, mut loss) = watch::channel(false);
    let closed = async {
        sender.send_replace(true);
    };

    assert_eq!(
        pending_close_error(&mut loss, closed).await,
        NodeRunnerError::ConnectionLost
    );
}

#[tokio::test]
async fn successful_close_cancels_a_pending_start_without_connection_loss() {
    let (_sender, mut loss) = watch::channel(false);

    assert_eq!(
        pending_close_error(&mut loss, std::future::ready(())).await,
        NodeRunnerError::Cancelled
    );
}
