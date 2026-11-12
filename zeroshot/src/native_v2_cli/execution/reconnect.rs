use std::future::{Future, pending};
use std::pin::Pin;
use std::time::Duration;

use openengine_cluster_protocol::RunId;
use tokio::time::{Instant, sleep, sleep_until};

use super::NativeV2CliError;

const INITIAL_DELAY: Duration = Duration::from_millis(100);
const MAX_DELAY: Duration = Duration::from_secs(5);
const RECONNECT_LIMIT: Duration = Duration::from_secs(10 * 60);

/// One policy for all target observation streams. Local controller retries keep their own lifetime.
pub(super) struct Reconnect<'a> {
    target: Option<&'a str>,
    run_id: &'a RunId,
    announced: bool,
    outage: Option<(Instant, NativeV2CliError)>,
    delay: Duration,
}

impl<'a> Reconnect<'a> {
    pub(super) fn new(target: Option<&'a str>, run_id: &'a RunId) -> Self {
        Self {
            target,
            run_id,
            announced: false,
            outage: None,
            delay: INITIAL_DELAY,
        }
    }

    pub(super) fn connected(&mut self) {
        self.outage = None;
        self.delay = INITIAL_DELAY;
    }

    /// Also bounds a reconnect request that never returns, rather than only its retry sleeps.
    pub(super) async fn expired(&mut self) -> NativeV2CliError {
        let Some((deadline, _)) = &self.outage else {
            return pending().await;
        };
        sleep_until(*deadline).await;
        self.timeout_error()
    }

    fn timeout_error(&mut self) -> NativeV2CliError {
        let (_, error) = self.outage.take().expect("an active outage has a deadline");
        NativeV2CliError::FollowConnectionLost {
            target: self
                .target
                .expect("only named targets have a deadline")
                .to_owned(),
            run_id: self.run_id.as_str().to_owned(),
            source: Box::new(error),
        }
    }

    /// Returns true on detach. The caller never stops or changes the target run.
    pub(super) async fn wait<D>(
        &mut self,
        error: NativeV2CliError,
        mut detach: Pin<&mut D>,
    ) -> Result<bool, NativeV2CliError>
    where
        D: Future<Output = ()> + ?Sized,
    {
        if !error.is_disconnected() {
            return Err(error);
        }
        if let Some(target) = self.target {
            if !self.announced {
                eprintln!(
                    "Connection to target {target:?} lost; run {} continues there. Reconnecting...",
                    self.run_id.as_str()
                );
                self.announced = true;
            }
            let deadline = self.outage.as_ref().map_or_else(
                || Instant::now() + RECONNECT_LIMIT,
                |(deadline, _)| *deadline,
            );
            self.outage = Some((deadline, error));
        }
        let delay = self.delay;
        if self.target.is_some() {
            self.delay = (delay * 2).min(MAX_DELAY);
        }
        tokio::select! {
            biased;
            () = detach.as_mut() => Ok(true),
            error = self.expired() => Err(error),
            () = sleep(delay) => Ok(false),
        }
    }
}
