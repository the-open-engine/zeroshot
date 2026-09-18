use std::io::Write;
use std::time::Duration;

use openengine_cluster_protocol::{RunAttachParams, SubscriptionCloseReason};

use super::super::{
    CliOutcome, CliSubscription, CliSubscriptionItem, DetachSignal, NativeV2CliBackend,
    NativeV2CliError,
};

use super::{SubscriptionStep, next_or_detach, retry_disconnected, write_json};

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
    loop {
        let mut subscription = tokio::select! {
            biased;
            () = &mut detach => return Ok(CliOutcome::Detached),
            result = backend.run_attach(target, params.clone()) => match result {
                Ok(subscription) => subscription,
                Err(error) if !opened => return Err(error),
                Err(error) => {
                    retry_disconnected(error).await?;
                    continue;
                }
            },
        };
        opened = true;
        loop {
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
                | SubscriptionStep::Item(None)
                | SubscriptionStep::Reconnect => break,
                SubscriptionStep::Item(Some(CliSubscriptionItem::Closed {
                    reason: SubscriptionCloseReason::SourceUnavailable,
                })) => {
                    return Err(NativeV2CliError::Protocol(
                        "observation source is unavailable; the stream is incomplete".to_owned(),
                    ));
                }
            }
        }
        tokio::select! {
            biased;
            () = &mut detach => return Ok(CliOutcome::Detached),
            () = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}
