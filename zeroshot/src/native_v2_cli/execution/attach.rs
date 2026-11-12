use std::io::Write;

use openengine_cluster_protocol::{RunAttachParams, SubscriptionCloseReason};

use super::super::{
    CliOutcome, CliSubscription, CliSubscriptionItem, DetachSignal, NativeV2CliBackend,
    NativeV2CliError,
};

use super::{Reconnect, SubscriptionStep, next_or_detach, write_json};

pub(super) struct RoutedAttach<'a> {
    pub(super) target: Option<&'a str>,
    pub(super) params: RunAttachParams,
}

pub(super) async fn follow_attach<B, S, W>(
    backend: &B,
    route: RoutedAttach<'_>,
    signal: &mut S,
    output: &mut W,
) -> Result<CliOutcome, NativeV2CliError>
where
    B: NativeV2CliBackend,
    S: DetachSignal,
    W: Write,
{
    let RoutedAttach { target, params } = route;
    let detach = signal.wait();
    tokio::pin!(detach);
    let mut opened = false;
    let mut reconnect = Reconnect::new(target, &params.run_id);
    loop {
        let mut subscription = tokio::select! {
            biased;
            () = &mut detach => return Ok(CliOutcome::Detached),
            error = reconnect.expired() => return Err(error),
            result = backend.run_attach(target, params.clone()) => match result {
                Ok(subscription) => {
                    reconnect.connected();
                    subscription
                },
                Err(error) if !opened => return Err(error),
                Err(error) => {
                    if reconnect.wait(error, detach.as_mut()).await? {
                        return Ok(CliOutcome::Detached);
                    }
                    continue;
                }
            },
        };
        opened = true;
        let error = loop {
            let step = next_or_detach(subscription.next(), detach.as_mut()).await?;
            match step {
                SubscriptionStep::Detached => return Ok(CliOutcome::Detached),
                SubscriptionStep::Item(Some(CliSubscriptionItem::Event(event))) => {
                    write_json(output, &event)?;
                }
                SubscriptionStep::Item(Some(CliSubscriptionItem::Closed {
                    reason: SubscriptionCloseReason::Done,
                })) => return Ok(CliOutcome::Completed),
                SubscriptionStep::Item(Some(CliSubscriptionItem::Closed {
                    reason: SubscriptionCloseReason::SlowConsumer,
                }))
                | SubscriptionStep::Item(None) => break NativeV2CliError::Disconnected,
                SubscriptionStep::Reconnect(error) => break error,
                SubscriptionStep::Item(Some(CliSubscriptionItem::Closed {
                    reason: SubscriptionCloseReason::SourceUnavailable,
                })) => {
                    return Err(NativeV2CliError::Protocol(
                        "observation source is unavailable; the stream is incomplete".to_owned(),
                    ));
                }
            }
        };
        if reconnect.wait(error, detach.as_mut()).await? {
            return Ok(CliOutcome::Detached);
        }
    }
}
