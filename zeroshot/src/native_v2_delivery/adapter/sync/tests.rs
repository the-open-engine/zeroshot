use super::*;

#[tokio::test]
async fn credential_refresh_respects_remaining_deadline() {
    let (_sender, receiver) = tokio::sync::watch::channel(false);
    let result = refresh_within_deadline(
        std::future::pending::<Result<(), EnvironmentRefreshError>>(),
        DriverCancellation::new(receiver),
        Duration::from_millis(1),
    )
    .await;

    assert!(matches!(result, Ok(CredentialRefreshProgress::TimedOut)));
}

#[tokio::test]
async fn credential_refresh_respects_cancellation() {
    let (sender, receiver) = tokio::sync::watch::channel(false);
    assert!(sender.send(true).is_ok());
    let result = refresh_within_deadline(
        std::future::pending::<Result<(), EnvironmentRefreshError>>(),
        DriverCancellation::new(receiver),
        Duration::from_secs(1),
    )
    .await;

    assert!(matches!(
        result,
        Err(DeliveryStop::Runner(NodeRunnerError::Cancelled))
    ));
}
